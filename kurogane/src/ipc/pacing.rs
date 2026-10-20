//! Pacing of a page's messages in flight to the browser.
//!
//! Queued messages leave once they are at least as many as those in flight,
//! so the browser starts on a round's first messages while the page makes
//! the rest. The browser's answers take messages out of flight. Nothing
//! waits on a time.

use std::collections::VecDeque;

use crate::ipc::envelope::FLAG_ANSWERS;

/// How a message is closed once it leaves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Closing {
    /// One call that left with nothing in flight, closed by its reply.
    Reply(u32),
    /// One other message that left with nothing in flight, closed by the
    /// first answers to a message sent after it, since the browser handles a
    /// frame's messages in order.
    Unasked,
    /// Closed by the browser's answers.
    Answers,
}

impl Closing {
    pub(crate) fn flags(self) -> u8 {
        match self {
            Closing::Reply(_) | Closing::Unasked => 0,
            Closing::Answers => FLAG_ANSWERS,
        }
    }
}

/// The message that left with nothing else in flight.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Alone {
    Call(u32),
    Message,
}

/// Messages in flight, which decide when queued messages leave.
#[derive(Default)]
pub(crate) struct Pacing {
    /// The message that left with nothing else in flight, until it closes.
    alone: Option<Alone>,
    /// Messages of each message that asked for answers, oldest first.
    unanswered: VecDeque<u32>,
}

impl Pacing {
    pub(crate) fn in_flight(&self) -> u32 {
        u32::from(self.alone.is_some()) + self.unanswered.iter().sum::<u32>()
    }

    /// Returns whether `queued` messages leave now, `full` when their batch
    /// takes no more.
    pub(crate) fn due(&self, queued: u32, full: bool) -> bool {
        queued > 0 && (full || queued >= self.in_flight())
    }

    /// Returns how a message holding `messages` that leaves now is closed.
    /// `call` is the id of its only message when that is a call.
    pub(crate) fn closing(&self, messages: u32, call: Option<u32>) -> Closing {
        if messages != 1 || self.in_flight() != 0 {
            return Closing::Answers;
        }
        match call {
            Some(id) => Closing::Reply(id),
            None => Closing::Unasked,
        }
    }

    /// Records a message holding `messages` that left.
    pub(crate) fn sent(&mut self, messages: u32, closing: Closing) {
        match closing {
            Closing::Reply(id) => self.alone = Some(Alone::Call(id)),
            Closing::Unasked => self.alone = Some(Alone::Message),
            Closing::Answers => self.unanswered.push_back(messages),
        }
    }

    /// Records answers that close the `closes` oldest messages that asked,
    /// and a lone message older than any of them.
    pub(crate) fn closed(&mut self, closes: u32) {
        if closes == 0 {
            return;
        }
        if self.alone == Some(Alone::Message) {
            self.alone = None;
        }
        let closes = (closes as usize).min(self.unanswered.len());
        self.unanswered.drain(..closes);
    }

    /// Records the reply to or the cancel of call `id`.
    pub(crate) fn answered(&mut self, id: u32) {
        if self.alone == Some(Alone::Call(id)) {
            self.alone = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Queues messages one at a time with nothing answered, as a burst does,
    /// and returns the sizes of those that left. `calls` when they are calls.
    fn burst(pacing: &mut Pacing, messages: u32, first_id: u32, calls: bool) -> Vec<u32> {
        let mut sizes = Vec::new();
        let mut queued = 0;
        for id in first_id..first_id + messages {
            queued += 1;
            if pacing.due(queued, false) {
                let call = (calls && queued == 1).then_some(id);
                let closing = pacing.closing(queued, call);
                pacing.sent(queued, closing);
                sizes.push(queued);
                queued = 0;
            }
        }
        if queued > 0 {
            sizes.push(queued);
        }
        sizes
    }

    #[test]
    fn a_call_with_nothing_in_flight_leaves_alone_and_its_reply_closes_it() {
        let mut pacing = Pacing::default();
        assert!(pacing.due(1, false));
        assert_eq!(pacing.closing(1, Some(7)), Closing::Reply(7));
        assert_eq!(Closing::Reply(7).flags(), 0, "no answers asked");
        pacing.sent(1, Closing::Reply(7));
        assert_eq!(pacing.in_flight(), 1);
        pacing.answered(8);
        assert_eq!(pacing.in_flight(), 1, "another id closes nothing");
        pacing.answered(7);
        assert_eq!(pacing.in_flight(), 0);
    }

    #[test]
    fn under_load_each_message_holds_what_is_in_flight() {
        for calls in [true, false] {
            let mut pacing = Pacing::default();
            assert_eq!(burst(&mut pacing, 16, 1, calls), [1, 1, 2, 4, 8]);
            assert_eq!(pacing.in_flight(), 16);
        }
    }

    #[test]
    fn only_messages_that_leave_with_others_in_flight_ask_for_answers() {
        let mut pacing = Pacing::default();
        assert_eq!(pacing.closing(1, None), Closing::Unasked, "one write alone");
        assert_eq!(Closing::Unasked.flags(), 0);
        assert_eq!(pacing.closing(2, Some(1)), Closing::Answers, "two at once");
        assert_eq!(Closing::Answers.flags(), FLAG_ANSWERS);
        pacing.sent(1, Closing::Reply(1));
        assert_eq!(
            pacing.closing(1, Some(2)),
            Closing::Answers,
            "one call while another is in flight"
        );
        assert_eq!(pacing.closing(1, None), Closing::Answers);
    }

    #[test]
    fn answers_to_a_later_message_close_a_lone_one() {
        let mut pacing = Pacing::default();
        pacing.sent(1, Closing::Unasked);
        assert_eq!(pacing.in_flight(), 1, "it counts until something closes it");
        pacing.answered(1);
        assert_eq!(pacing.in_flight(), 1, "no call of that id");
        pacing.closed(0);
        assert_eq!(pacing.in_flight(), 1, "a delivery that closes nothing");
        pacing.sent(1, Closing::Answers);
        pacing.closed(1);
        assert_eq!(
            pacing.in_flight(),
            0,
            "the later message's answers close both"
        );
    }

    #[test]
    fn a_lone_call_stays_until_its_own_reply() {
        let mut pacing = Pacing::default();
        pacing.sent(1, Closing::Reply(5));
        pacing.sent(1, Closing::Answers);
        pacing.closed(1);
        assert_eq!(
            pacing.in_flight(),
            1,
            "answers to another message leave the call"
        );
        pacing.answered(5);
        assert_eq!(pacing.in_flight(), 0);
    }

    #[test]
    fn answers_close_the_oldest_messages_and_never_more_than_left() {
        let mut pacing = Pacing::default();
        pacing.sent(2, Closing::Answers);
        pacing.sent(3, Closing::Answers);
        pacing.closed(1);
        assert_eq!(pacing.in_flight(), 3);
        pacing.closed(5);
        assert_eq!(pacing.in_flight(), 0);
    }

    #[test]
    fn a_call_that_never_returns_holds_no_other_back() {
        let mut pacing = Pacing::default();
        // A long-poll leaves alone and is never answered
        pacing.sent(1, Closing::Reply(1));
        assert!(pacing.due(1, false));
        // Another message leaves asking for answers, a third waits for them
        pacing.sent(1, Closing::Answers);
        assert!(!pacing.due(1, false));
        pacing.closed(1);
        assert!(pacing.due(1, false));
    }

    #[test]
    fn once_every_asking_message_is_answered_any_queued_message_leaves() {
        let mut pacing = Pacing::default();
        pacing.sent(1, Closing::Reply(1));
        burst(&mut pacing, 40, 2, false);
        assert!(pacing.in_flight() > 1);
        pacing.closed(u32::MAX);
        for queued in 1..64 {
            assert!(pacing.due(queued, false), "{queued} queued messages wait");
        }
    }

    #[test]
    fn a_full_batch_leaves_whatever_is_in_flight() {
        let mut pacing = Pacing::default();
        pacing.sent(30, Closing::Answers);
        assert!(!pacing.due(2, false));
        assert!(pacing.due(2, true));
        assert!(!pacing.due(0, true), "nothing queued, nothing leaves");
    }
}
