//! The browser's messages on their way to a renderer.
//!
//! Each frame's messages leave in the order sent. Those sent while one of a
//! page's messages is handled leave once it is done, each frame's as one
//! message, early when a batch fills. The sender's carries the answer that
//! closes the page's message when it asked for one, with the closes that wait.
//! Those sent off the UI thread wait for a task on the UI thread, which sends
//! each frame's together. Other threads' messages queued before a handling
//! leave before its messages, those queued during it after them, so a thread
//! a handler starts never overtakes what the handler sent first.

use std::cell::RefCell;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Mutex, OnceLock};

use tanso::*;
use tracing::debug;

use crate::ipc::batch::{self, Batch};
use crate::ipc::envelope::Envelope;
use crate::ipc::transport::message::build_message;
use crate::ipc::FrameId;

/// What leaves for a frame now.
enum Leaving<'a> {
    /// Messages that leave together, closing `closes` of the page's messages.
    Together { messages: &'a Batch, closes: u32 },
    /// One message too large for a batch.
    Alone {
        envelope: &'a Envelope,
        payload: &'a [u8],
    },
}

/// A frame's messages that leave together.
struct Waiting<F> {
    frame_id: FrameId,
    frame: F,
    messages: Batch,
    closes: u32,
}

/// Messages for several frames, each frame's to leave as one. Generic over
/// the frame so the grouping can be tested without CEF.
struct Outgoing<F> {
    frames: Vec<Waiting<F>>,
}

impl<F: Clone> Outgoing<F> {
    fn new() -> Self {
        Self { frames: Vec::new() }
    }

    fn waiting(&mut self, frame_id: &FrameId, frame: &F) -> &mut Waiting<F> {
        let index = match self.frames.iter().position(|w| w.frame_id == *frame_id) {
            Some(index) => index,
            None => {
                self.frames.push(Waiting {
                    frame_id: frame_id.clone(),
                    frame: frame.clone(),
                    messages: Batch::default(),
                    closes: 0,
                });
                self.frames.len() - 1
            }
        };
        &mut self.frames[index]
    }

    /// Adds a message for `frame`. A batch that fills leaves at once; a
    /// message too large for one leaves at once after the frame's others.
    fn push(
        &mut self,
        frame_id: &FrameId,
        frame: &F,
        envelope: &Envelope,
        payload: &[u8],
        leave: &mut impl FnMut(&F, Leaving<'_>),
    ) {
        let waiting = self.waiting(frame_id, frame);
        if waiting.messages.push(envelope, &[payload]) {
            if waiting.messages.is_full() {
                waiting.leave_messages(leave);
            }
            return;
        }
        waiting.leave_messages(leave);
        leave(&waiting.frame, Leaving::Alone { envelope, payload });
    }

    /// Counts one of the page's messages from `frame` closed.
    fn close(&mut self, frame_id: &FrameId, frame: &F) {
        self.waiting(frame_id, frame).closes += 1;
    }

    /// Returns whether messages wait for `frame_id`.
    fn has_messages(&self, frame_id: &FrameId) -> bool {
        self.frames
            .iter()
            .any(|w| w.frame_id == *frame_id && !w.messages.is_empty())
    }

    /// Sends the messages waiting for `frame_id`, keeping its closes.
    fn leave_frame(&mut self, frame_id: &FrameId, leave: &mut impl FnMut(&F, Leaving<'_>)) {
        if let Some(waiting) = self.frames.iter_mut().find(|w| w.frame_id == *frame_id) {
            waiting.leave_messages(leave);
        }
    }

    /// Sends every frame's waiting messages, keeping their closes.
    fn leave_messages(&mut self, leave: &mut impl FnMut(&F, Leaving<'_>)) {
        for waiting in &mut self.frames {
            waiting.leave_messages(leave);
        }
    }

    /// Sends every frame's waiting messages with its closes, in the order
    /// the frames came.
    fn finish(self, leave: &mut impl FnMut(&F, Leaving<'_>)) {
        for waiting in self.frames {
            if !waiting.messages.is_empty() || waiting.closes > 0 {
                leave(
                    &waiting.frame,
                    Leaving::Together {
                        messages: &waiting.messages,
                        closes: waiting.closes,
                    },
                );
            }
        }
    }
}

impl<F> Waiting<F> {
    /// Sends the waiting messages, keeping the closes.
    fn leave_messages(&mut self, leave: &mut impl FnMut(&F, Leaving<'_>)) {
        if self.messages.is_empty() {
            return;
        }
        leave(
            &self.frame,
            Leaving::Together {
                messages: &self.messages,
                closes: 0,
            },
        );
        self.messages.clear();
    }
}

/// Sends what leaves for `frame`, after what waits for the UI thread's task.
fn send_leaving(frame: &Frame, leaving: Leaving<'_>) {
    send_laters(take_waiting());
    send_now(frame, leaving);
}

/// Sends what leaves for `frame` now.
fn send_now(frame: &Frame, leaving: Leaving<'_>) {
    let msg = match leaving {
        Leaving::Together { messages, closes } => messages.build(0, closes),
        Leaving::Alone { envelope, payload } => build_message(envelope, payload),
    };
    match msg {
        Some(mut msg) => frame.send_process_message(ProcessId::RENDERER, Some(&mut msg)),
        None => debug!("[IPC Browser] failed to build a message"),
    }
}

/// What a scope collects.
struct Collecting {
    sender_id: FrameId,
    sender: Frame,
    /// Whether the page's message asked for answers.
    answers: bool,
    outgoing: Outgoing<Frame>,
}

thread_local! {
    static COLLECTING: RefCell<Option<Collecting>> = const { RefCell::new(None) };
}

/// Collects what the browser sends while it handles one of a page's
/// messages. On drop each frame's leaves as one message, the sender's with
/// the answer that closes the page's message when it asked for one. When
/// nothing went to the sender that answer leaves from the UI thread's next
/// task, beside the others made meanwhile. A message handled inside another's
/// handling, as in a nested message loop, has its own scope; the outer one's
/// messages leave first and it resumes when the inner one ends.
#[must_use]
pub(crate) struct Scope {
    /// The collecting this one interrupted.
    outer: Option<Collecting>,
}

impl Scope {
    /// Begins collecting for a message from `sender`, `answers` when it
    /// asked for answers.
    pub(crate) fn begin(sender: &Frame, sender_id: FrameId, answers: bool) -> Self {
        let collecting = Collecting {
            sender_id,
            sender: sender.clone(),
            answers,
            outgoing: Outgoing::new(),
        };
        let outer = COLLECTING.with(|c| c.borrow_mut().replace(collecting));
        let outer = outer.map(|mut outer| {
            outer.outgoing.leave_messages(&mut send_now);
            outer
        });
        // What other threads queued before this handling leaves before it
        send_laters(take_waiting());
        Self { outer }
    }
}

impl Drop for Scope {
    fn drop(&mut self) {
        let outer = self.outer.take();
        let Some(collecting) = COLLECTING.with(|c| std::mem::replace(&mut *c.borrow_mut(), outer))
        else {
            return;
        };
        let Collecting {
            sender_id,
            sender,
            answers,
            mut outgoing,
        } = collecting;
        // A close with nothing to ride with waits for the UI thread's next
        // task, where other closes and messages may join it
        let close_later = answers && !outgoing.has_messages(&sender_id);
        if answers && !close_later {
            outgoing.close(&sender_id, &sender);
        }
        // What other threads queued meanwhile leaves after this handling's
        // messages, but closes among it ride with them
        let waiting = take_waiting();
        let mut rest = Vec::with_capacity(waiting.len());
        for later in waiting {
            match later {
                Later::Close { frame, frame_id } if outgoing.has_messages(&frame_id) => {
                    outgoing.close(&frame_id, &frame);
                }
                later => rest.push(later),
            }
        }
        outgoing.finish(&mut send_now);
        send_laters(rest);
        if close_later {
            later(Later::Close {
                frame: sender,
                frame_id: sender_id,
            });
        }
    }
}

/// Sends `frame` a message whose payload is `payload`. It joins the scope
/// collecting on this thread, or off the UI thread waits for the UI thread's
/// next task, or else leaves at once after what waits.
pub(crate) fn send(frame: &Frame, frame_id: &FrameId, envelope: &Envelope, payload: &[u8]) {
    if collect(frame, frame_id, envelope, payload) {
        return;
    }
    if currently_on(ThreadId::UI) == 0 {
        later_message(frame, frame_id, envelope, payload.to_vec());
        return;
    }
    send_leaving(frame, Leaving::Alone { envelope, payload });
}

/// As [`send`], for a payload the caller hands over, which spares a copy off
/// the UI thread.
pub(crate) fn send_owned(frame: &Frame, frame_id: &FrameId, envelope: &Envelope, payload: Vec<u8>) {
    if collect(frame, frame_id, envelope, &payload) {
        return;
    }
    if currently_on(ThreadId::UI) == 0 {
        later_message(frame, frame_id, envelope, payload);
        return;
    }
    send_leaving(
        frame,
        Leaving::Alone {
            envelope,
            payload: &payload,
        },
    );
}

/// Adds a message to the scope collecting on this thread. Returns false when
/// none is.
fn collect(frame: &Frame, frame_id: &FrameId, envelope: &Envelope, payload: &[u8]) -> bool {
    COLLECTING.with(|c| match c.borrow_mut().as_mut() {
        Some(collecting) => {
            collecting
                .outgoing
                .push(frame_id, frame, envelope, payload, &mut send_now);
            true
        }
        None => false,
    })
}

/// Closes a message from `frame` that could not be read. Closing one that
/// asked for no answers only lets the page's queued messages leave sooner.
pub(crate) fn close_unread(frame: &Frame, frame_id: FrameId) {
    later(Later::Close {
        frame: frame.clone(),
        frame_id,
    });
}

/// Something for a frame that waits for the UI thread's task.
enum Later {
    /// A message that joins its frame's batch.
    Message {
        frame: Frame,
        frame_id: FrameId,
        envelope: Envelope,
        payload: Vec<u8>,
    },
    /// A message too large for a batch, built off the UI thread.
    Built {
        frame: Frame,
        frame_id: FrameId,
        message: ProcessMessage,
    },
    /// The close of a page's message whose handling sent its frame nothing.
    Close { frame: Frame, frame_id: FrameId },
}

/// Queues a message made off the UI thread. One too large for a batch is
/// built here, which keeps its copy off the UI thread.
fn later_message(frame: &Frame, frame_id: &FrameId, envelope: &Envelope, payload: Vec<u8>) {
    let (frame, frame_id) = (frame.clone(), frame_id.clone());
    if batch::fits(&[&payload]) {
        later(Later::Message {
            frame,
            frame_id,
            envelope: *envelope,
            payload,
        });
        return;
    }
    match build_message(envelope, &payload) {
        Some(message) => later(Later::Built {
            frame,
            frame_id,
            message,
        }),
        None => debug!("[IPC Browser] failed to build a message"),
    }
}

/// What waits for the UI thread's task. Unbounded like CEF's own queue for
/// messages sent off the UI thread, which it stands in for; a thread that
/// outruns the UI thread grows it as it grew CEF's.
struct Laters {
    send: Sender<Later>,
    /// Read on the UI thread alone.
    receive: Mutex<Receiver<Later>>,
}

fn laters() -> &'static Laters {
    static LATERS: OnceLock<Laters> = OnceLock::new();
    LATERS.get_or_init(|| {
        let (send, receive) = channel();
        Laters {
            send,
            receive: Mutex::new(receive),
        }
    })
}

/// Queues `later` for the UI thread and posts one task for it.
fn later(later: Later) {
    // The receiver lives in a static, so a send never fails
    if laters().send.send(later).is_err() {
        return;
    }
    let mut task = LaterTask::new();
    post_task(ThreadId::UI, Some(&mut task));
}

/// Takes everything that waits for the UI thread's task, in the order queued.
fn take_waiting() -> Vec<Later> {
    match laters().receive.lock() {
        Ok(receive) => receive.try_iter().collect(),
        Err(_) => Vec::new(),
    }
}

/// Sends `waiting`, each frame's as few messages as its sizes allow, in
/// order.
fn send_laters(waiting: Vec<Later>) {
    if waiting.is_empty() {
        return;
    }
    let mut outgoing = Outgoing::new();
    for later in waiting {
        match later {
            Later::Message {
                frame,
                frame_id,
                envelope,
                payload,
            } => outgoing.push(&frame_id, &frame, &envelope, &payload, &mut send_now),
            Later::Built {
                frame,
                frame_id,
                mut message,
            } => {
                outgoing.leave_frame(&frame_id, &mut send_now);
                frame.send_process_message(ProcessId::RENDERER, Some(&mut message));
            }
            Later::Close { frame, frame_id } => outgoing.close(&frame_id, &frame),
        }
    }
    outgoing.finish(&mut send_now);
}

wrap_task! {
    struct LaterTask {}

    impl Task {
        fn execute(&self) {
            // A panic must not unwind across this CEF callback
            if std::panic::catch_unwind(|| send_laters(take_waiting())).is_err() {
                debug!("[IPC Browser] sending waiting messages panicked");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::batch::{ALONE_MIN, BATCH_MAX};
    use crate::ipc::envelope::*;

    /// What left for a frame, the ids it held with the closes it carried, or
    /// one message alone.
    #[derive(Debug, PartialEq)]
    enum Left {
        Together(&'static str, Vec<u32>, u32),
        Alone(&'static str, u32),
    }

    fn envelope(id: u32) -> Envelope {
        Envelope {
            version: ENVELOPE_VERSION,
            subsystem: SUB_EVENT,
            opcode: EVENT_EMIT,
            flags: 0,
            correlation_id: id,
            payload_kind: PAYLOAD_BINARY,
        }
    }

    fn recorder(left: &mut Vec<Left>) -> impl FnMut(&&'static str, Leaving<'_>) + '_ {
        |frame, leaving| {
            left.push(match leaving {
                Leaving::Together { messages, closes } => Left::Together(
                    frame,
                    messages.iter().map(|(e, _)| e.correlation_id).collect(),
                    closes,
                ),
                Leaving::Alone { envelope, .. } => Left::Alone(frame, envelope.correlation_id),
            })
        }
    }

    fn push(
        outgoing: &mut Outgoing<&'static str>,
        frame: &'static str,
        id: u32,
        size: usize,
        left: &mut Vec<Left>,
    ) {
        outgoing.push(
            &FrameId::new(frame),
            &frame,
            &envelope(id),
            &vec![0; size],
            &mut recorder(left),
        );
    }

    #[test]
    fn each_frame_leaves_as_one_message_in_the_order_the_frames_came() {
        let mut left = Vec::new();
        let mut outgoing = Outgoing::new();
        push(&mut outgoing, "b", 1, 10, &mut left);
        push(&mut outgoing, "a", 2, 10, &mut left);
        push(&mut outgoing, "b", 3, 10, &mut left);
        assert!(left.is_empty(), "nothing leaves before the end");
        outgoing.finish(&mut recorder(&mut left));
        assert_eq!(
            left,
            [
                Left::Together("b", vec![1, 3], 0),
                Left::Together("a", vec![2], 0)
            ]
        );
    }

    #[test]
    fn a_full_batch_leaves_at_once_and_the_rest_at_the_end() {
        let mut left = Vec::new();
        let mut outgoing = Outgoing::new();
        let size = ALONE_MIN / 2;
        let full = BATCH_MAX.div_ceil(4 + ENVELOPE_SIZE + size) as u32;
        for id in 1..=full + 1 {
            push(&mut outgoing, "a", id, size, &mut left);
        }
        assert_eq!(left, [Left::Together("a", (1..=full).collect(), 0)]);
        outgoing.finish(&mut recorder(&mut left));
        assert_eq!(left[1..], [Left::Together("a", vec![full + 1], 0)]);
    }

    #[test]
    fn a_message_too_large_for_a_batch_leaves_after_its_frames_others() {
        let mut left = Vec::new();
        let mut outgoing = Outgoing::new();
        push(&mut outgoing, "a", 1, 10, &mut left);
        push(&mut outgoing, "b", 2, 10, &mut left);
        push(&mut outgoing, "a", 3, ALONE_MIN, &mut left);
        assert_eq!(
            left,
            [Left::Together("a", vec![1], 0), Left::Alone("a", 3)],
            "the other frame's message waits"
        );
        outgoing.finish(&mut recorder(&mut left));
        assert_eq!(left[2..], [Left::Together("b", vec![2], 0)]);
    }

    #[test]
    fn closes_ride_with_their_frames_messages_or_alone() {
        let mut left = Vec::new();
        let mut outgoing = Outgoing::new();
        outgoing.close(&FrameId::new("a"), &"a");
        push(&mut outgoing, "b", 1, 10, &mut left);
        outgoing.close(&FrameId::new("b"), &"b");
        outgoing.close(&FrameId::new("b"), &"b");
        assert!(outgoing.has_messages(&FrameId::new("b")));
        assert!(!outgoing.has_messages(&FrameId::new("a")));
        outgoing.finish(&mut recorder(&mut left));
        assert_eq!(
            left,
            [
                Left::Together("a", vec![], 1),
                Left::Together("b", vec![1], 2)
            ]
        );
    }

    #[test]
    fn leaving_early_keeps_the_closes_for_the_end() {
        let mut left = Vec::new();
        let mut outgoing = Outgoing::new();
        push(&mut outgoing, "a", 1, 10, &mut left);
        outgoing.close(&FrameId::new("a"), &"a");
        outgoing.leave_messages(&mut recorder(&mut left));
        push(&mut outgoing, "a", 2, 10, &mut left);
        outgoing.leave_frame(&FrameId::new("a"), &mut recorder(&mut left));
        assert_eq!(
            left,
            [
                Left::Together("a", vec![1], 0),
                Left::Together("a", vec![2], 0)
            ]
        );
        outgoing.finish(&mut recorder(&mut left));
        assert_eq!(left[2..], [Left::Together("a", vec![], 1)]);
    }

    #[test]
    fn a_frame_with_nothing_left_sends_nothing() {
        let mut left = Vec::new();
        let mut outgoing = Outgoing::new();
        push(&mut outgoing, "a", 1, 10, &mut left);
        outgoing.leave_messages(&mut recorder(&mut left));
        outgoing.finish(&mut recorder(&mut left));
        assert_eq!(left, [Left::Together("a", vec![1], 0)]);
    }
}
