//! The renderer's calls on their way to the browser.
//!
//! A frame's queued calls leave once they are at least as many as its calls
//! in flight. A call leaves at once when nothing is in flight. Under load
//! each message holds as many calls as are still in flight, so the browser
//! starts on a round's first calls while the page makes the rest. A message
//! that leaves while others are in flight asks the browser for answers that
//! close it (`FLAG_ANSWERS`). A call that leaves alone is closed by its own
//! reply. A call too large for a batch leaves at once outside the pacing,
//! since copying it costs more than its message. Any other message of a
//! frame sends the frame's queued calls first, so the browser sees the
//! page's order.

use std::cell::RefCell;
use std::collections::VecDeque;

use tanso::*;

use crate::ipc::batch::Batch;
use crate::ipc::envelope::{Envelope, FLAG_ANSWERS};
use crate::ipc::transport::message::build_message_parts;
use crate::ipc::FrameId;

/// A frame's calls in flight, which decide when its queued calls leave.
#[derive(Default)]
struct Pacing {
    /// The call that left with nothing else in flight, until its reply or
    /// its cancel.
    alone: Option<u32>,
    /// Calls of each message that asked for answers, oldest first.
    unanswered: VecDeque<u32>,
}

impl Pacing {
    fn in_flight(&self) -> u32 {
        u32::from(self.alone.is_some()) + self.unanswered.iter().sum::<u32>()
    }

    /// Returns whether `queued` calls leave now, `full` when their batch
    /// takes no more.
    fn due(&self, queued: u32, full: bool) -> bool {
        queued > 0 && (full || queued >= self.in_flight())
    }

    /// Returns the flags of a message of `calls` that leaves now. One call
    /// with nothing in flight leaves alone, any other message asks for
    /// answers.
    fn flags(&self, calls: u32) -> u8 {
        if calls == 1 && self.in_flight() == 0 {
            0
        } else {
            FLAG_ANSWERS
        }
    }

    /// Records a message of `calls` that left with `flags`, `first` its
    /// first call's id.
    fn sent(&mut self, calls: u32, first: u32, flags: u8) {
        if flags & FLAG_ANSWERS == 0 {
            self.alone = Some(first);
        } else {
            self.unanswered.push_back(calls);
        }
    }

    /// Records answers that close the `closes` oldest messages that asked.
    fn closed(&mut self, closes: u32) {
        let closes = (closes as usize).min(self.unanswered.len());
        self.unanswered.drain(..closes);
    }

    /// Records the reply to or the cancel of call `id`.
    fn answered(&mut self, id: u32) {
        if self.alone == Some(id) {
            self.alone = None;
        }
    }
}

/// A frame's calls on their way.
struct Outbox {
    frame_id: FrameId,
    frame: Frame,
    calls: Batch,
    pacing: Pacing,
}

impl Outbox {
    fn due(&self) -> bool {
        self.pacing.due(self.calls.len(), self.calls.is_full())
    }

    /// Sends the queued calls as one message.
    fn send_calls(&mut self) {
        let calls = std::mem::take(&mut self.calls);
        let Some(first) = calls.first_id() else {
            return;
        };
        let flags = self.pacing.flags(calls.len());
        if let Some(mut msg) = calls.build("kurogane_rpc", flags) {
            self.frame
                .send_process_message(ProcessId::BROWSER, Some(&mut msg));
            self.pacing.sent(calls.len(), first, flags);
        }
    }
}

thread_local! {
    static OUTBOXES: RefCell<Vec<Outbox>> = const { RefCell::new(Vec::new()) };
}

fn with_outbox<R>(frame: &Frame, frame_id: &FrameId, f: impl FnOnce(&mut Outbox) -> R) -> R {
    OUTBOXES.with(|outboxes| {
        let mut outboxes = outboxes.borrow_mut();
        let index = match outboxes.iter().position(|o| o.frame_id == *frame_id) {
            Some(index) => index,
            None => {
                outboxes.push(Outbox {
                    frame_id: frame_id.clone(),
                    frame: frame.clone(),
                    calls: Batch::default(),
                    pacing: Pacing::default(),
                });
                outboxes.len() - 1
            }
        };
        f(&mut outboxes[index])
    })
}

/// Queues a call in its frame's batch and sends the batch when it is due.
/// `None` when the call's message cannot be built.
pub(crate) fn send_call(
    frame: &Frame,
    frame_id: &FrameId,
    envelope: &Envelope,
    parts: &[&[u8]],
) -> Option<()> {
    with_outbox(frame, frame_id, |outbox| {
        if outbox.calls.push(envelope, parts) {
            if outbox.due() {
                outbox.send_calls();
            }
            return Some(());
        }
        // A call too large for a batch leaves after the queued calls
        outbox.send_calls();
        let mut msg = build_message_parts("kurogane_rpc", envelope, parts)?;
        outbox
            .frame
            .send_process_message(ProcessId::BROWSER, Some(&mut msg));
        Some(())
    })
}

/// Runs `deliver`, a delivery of one of the browser's messages, then sends
/// the batches that are due. `answered` names the frame whose oldest
/// messages of calls the delivered answers close, with how many.
pub(crate) fn delivering<R>(answered: Option<(FrameId, u32)>, deliver: impl FnOnce() -> R) -> R {
    if let Some((frame_id, closes)) = &answered {
        OUTBOXES.with(|outboxes| {
            if let Some(outbox) = outboxes
                .borrow_mut()
                .iter_mut()
                .find(|o| o.frame_id == *frame_id)
            {
                outbox.pacing.closed(*closes);
            }
        });
    }
    let result = deliver();
    OUTBOXES.with(|outboxes| {
        for outbox in outboxes.borrow_mut().iter_mut() {
            if outbox.due() {
                outbox.send_calls();
            }
        }
    });
    result
}

/// Records the reply to or the cancel of a call of `frame_id`.
pub(crate) fn answered(frame_id: &FrameId, id: u32) {
    OUTBOXES.with(|outboxes| {
        if let Some(outbox) = outboxes
            .borrow_mut()
            .iter_mut()
            .find(|o| o.frame_id == *frame_id)
        {
            outbox.pacing.answered(id);
        }
    });
}

/// Sends `msg` after the frame's queued calls.
pub(crate) fn send(frame: &Frame, msg: &mut ProcessMessage) {
    // The frame's id is looked up only when some call waits
    let waiting = OUTBOXES.with(|outboxes| outboxes.borrow().iter().any(|o| !o.calls.is_empty()));
    if waiting {
        flush_frame(&FrameId::of(frame));
    }
    frame.send_process_message(ProcessId::BROWSER, Some(msg));
}

/// Sends a frame's queued calls.
fn flush_frame(frame_id: &FrameId) {
    OUTBOXES.with(|outboxes| {
        if let Some(outbox) = outboxes
            .borrow_mut()
            .iter_mut()
            .find(|o| o.frame_id == *frame_id)
        {
            outbox.send_calls();
        }
    });
}

/// Sends a frame's queued calls and forgets the frame.
pub(crate) fn release_frame(frame_id: &FrameId) {
    flush_frame(frame_id);
    OUTBOXES.with(|outboxes| outboxes.borrow_mut().retain(|o| o.frame_id != *frame_id));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Queues calls one at a time with nothing answered, as a burst does, and
    /// returns the sizes of the messages that left.
    fn burst(pacing: &mut Pacing, calls: u32, first_id: u32) -> Vec<u32> {
        let mut sizes = Vec::new();
        let mut queued = 0;
        for id in first_id..first_id + calls {
            queued += 1;
            if pacing.due(queued, false) {
                let flags = pacing.flags(queued);
                pacing.sent(queued, id + 1 - queued, flags);
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
        assert_eq!(pacing.flags(1), 0, "no answers asked");
        pacing.sent(1, 7, 0);
        assert_eq!(pacing.in_flight(), 1);
        pacing.answered(8);
        assert_eq!(pacing.in_flight(), 1, "another id closes nothing");
        pacing.answered(7);
        assert_eq!(pacing.in_flight(), 0);
    }

    #[test]
    fn under_load_each_message_holds_what_is_in_flight() {
        let mut pacing = Pacing::default();
        assert_eq!(burst(&mut pacing, 16, 1), [1, 1, 2, 4, 8]);
        assert_eq!(pacing.in_flight(), 16);
    }

    #[test]
    fn every_message_but_a_lone_call_asks_for_answers() {
        let mut pacing = Pacing::default();
        assert_eq!(pacing.flags(2), FLAG_ANSWERS, "two calls at once");
        pacing.sent(1, 1, 0);
        assert_eq!(
            pacing.flags(1),
            FLAG_ANSWERS,
            "one call while another is in flight"
        );
    }

    #[test]
    fn answers_close_the_oldest_messages_and_never_more_than_left() {
        let mut pacing = Pacing::default();
        pacing.sent(2, 1, FLAG_ANSWERS);
        pacing.sent(3, 3, FLAG_ANSWERS);
        pacing.closed(1);
        assert_eq!(pacing.in_flight(), 3);
        pacing.closed(5);
        assert_eq!(pacing.in_flight(), 0);
    }

    #[test]
    fn a_call_that_never_returns_holds_no_other_back() {
        let mut pacing = Pacing::default();
        // A long-poll leaves alone and is never answered
        pacing.sent(1, 1, 0);
        assert!(pacing.due(1, false));
        // Another call leaves asking for answers, a third waits for them
        pacing.sent(1, 2, FLAG_ANSWERS);
        assert!(!pacing.due(1, false));
        pacing.closed(1);
        assert!(pacing.due(1, false));
    }

    #[test]
    fn once_every_asking_message_is_answered_any_queued_call_leaves() {
        let mut pacing = Pacing::default();
        pacing.sent(1, 1, 0);
        burst(&mut pacing, 40, 2);
        assert!(pacing.in_flight() > 1);
        pacing.closed(u32::MAX);
        for queued in 1..64 {
            assert!(pacing.due(queued, false), "{queued} queued calls wait");
        }
    }

    #[test]
    fn a_full_batch_leaves_whatever_is_in_flight() {
        let mut pacing = Pacing::default();
        pacing.sent(30, 1, FLAG_ANSWERS);
        assert!(!pacing.due(2, false));
        assert!(pacing.due(2, true));
        assert!(!pacing.due(0, true), "nothing queued, nothing leaves");
    }
}
