//! The page's messages on their way to the browser.
//!
//! Each frame's messages leave in the order the page sent them, paced by its
//! messages in flight (`pacing`). A message leaves at once when nothing is
//! in flight and asks for no answers. A message that leaves while others are
//! in flight asks the browser for answers that close it (`FLAG_ANSWERS`). A
//! message too large for a batch leaves at once outside the pacing, since
//! copying it costs more than its message. A leaving page's messages leave
//! at once outside the pacing, since the page may be gone before the answers
//! that pace them.

use std::cell::RefCell;

use tanso::*;

use crate::ipc::batch::{self, Batch};
use crate::ipc::envelope::{Envelope, RPC_INVOKE, SUB_RPC};
use crate::ipc::pacing::{Closing, Pacing};
use crate::ipc::transport::message::build_message_parts;
use crate::ipc::FrameId;

/// Returns the id of the call `envelope` makes.
fn call_id(envelope: &Envelope) -> Option<u32> {
    (envelope.subsystem == SUB_RPC && envelope.opcode == RPC_INVOKE)
        .then_some(envelope.correlation_id)
}

/// A frame's messages on their way.
struct Outbox {
    frame_id: FrameId,
    frame: Frame,
    queue: Batch,
    pacing: Pacing,
    /// Whether the page is leaving.
    leaving: bool,
}

impl Outbox {
    fn due(&self) -> bool {
        self.pacing.due(self.queue.len(), self.queue.is_full())
    }

    /// Sends the queued messages as one.
    fn send_queue(&mut self) {
        let Some((first, _)) = self.queue.iter().next() else {
            return;
        };
        let messages = self.queue.len();
        let closing = self.pacing.closing(messages, call_id(&first));
        // CEF builds no message only out of memory, where the process aborts
        // at its next allocation, so the queued calls are not rejected
        if let Some(mut msg) = self.queue.build(closing.flags(), 0) {
            self.frame
                .send_process_message(ProcessId::BROWSER, Some(&mut msg));
            self.pacing.sent(messages, closing);
        }
        self.queue.clear();
    }

    /// Sends one message straight from its parts, paced or not. `None` when
    /// it cannot be built.
    fn send_alone(&mut self, envelope: &Envelope, parts: &[&[u8]], paced: bool) -> Option<()> {
        let closing = paced.then(|| self.pacing.closing(1, call_id(envelope)));
        let mut envelope = *envelope;
        envelope.flags |= closing.map_or(0, Closing::flags);
        let mut msg = build_message_parts(&envelope, parts)?;
        self.frame
            .send_process_message(ProcessId::BROWSER, Some(&mut msg));
        if let Some(closing) = closing {
            self.pacing.sent(1, closing);
        }
        Some(())
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
                    queue: Batch::default(),
                    pacing: Pacing::default(),
                    leaving: false,
                });
                outboxes.len() - 1
            }
        };
        f(&mut outboxes[index])
    })
}

/// Sends a message of `frame`'s page whose payload is `parts`, after the
/// frame's queued messages, now or once it is due. `None` when it cannot be
/// built.
pub(crate) fn send(
    frame: &Frame,
    frame_id: &FrameId,
    envelope: &Envelope,
    parts: &[&[u8]],
) -> Option<()> {
    with_outbox(frame, frame_id, |outbox| {
        if outbox.leaving || !batch::fits(parts) {
            outbox.send_queue();
            return outbox.send_alone(envelope, parts, false);
        }
        // A due message with nothing queued leaves straight from its parts
        if outbox.queue.is_empty() && outbox.pacing.due(1, false) {
            return outbox.send_alone(envelope, parts, true);
        }
        outbox.queue.push(envelope, parts);
        if outbox.due() {
            outbox.send_queue();
        }
        Some(())
    })
}

/// Records whether `frame`'s page is leaving and sends its queued messages.
pub(crate) fn leaving(frame: &Frame, frame_id: &FrameId, leaving: bool) {
    with_outbox(frame, frame_id, |outbox| {
        outbox.leaving = leaving;
        outbox.send_queue();
    });
}

/// Sends `msg`, a message outside the IPC protocol, after the frame's queued
/// messages.
pub(crate) fn send_after(frame: &Frame, frame_id: &FrameId, msg: &mut ProcessMessage) {
    flush_frame(frame_id);
    frame.send_process_message(ProcessId::BROWSER, Some(msg));
}

/// Runs `deliver`, the delivery of one of the browser's messages to
/// `addressed`, which closes `closes` of its oldest messages, then sends the
/// queues that are due.
pub(crate) fn delivering<R>(addressed: &FrameId, closes: u32, deliver: impl FnOnce() -> R) -> R {
    if closes > 0 {
        OUTBOXES.with(|outboxes| {
            if let Some(outbox) = outboxes
                .borrow_mut()
                .iter_mut()
                .find(|o| o.frame_id == *addressed)
            {
                outbox.pacing.closed(closes);
            }
        });
    }
    let result = deliver();
    OUTBOXES.with(|outboxes| {
        for outbox in outboxes.borrow_mut().iter_mut() {
            if outbox.due() {
                outbox.send_queue();
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

/// Sends a frame's queued messages.
fn flush_frame(frame_id: &FrameId) {
    OUTBOXES.with(|outboxes| {
        if let Some(outbox) = outboxes
            .borrow_mut()
            .iter_mut()
            .find(|o| o.frame_id == *frame_id)
        {
            outbox.send_queue();
        }
    });
}

/// Sends a frame's queued messages and forgets the frame.
pub(crate) fn release_frame(frame_id: &FrameId) {
    flush_frame(frame_id);
    OUTBOXES.with(|outboxes| outboxes.borrow_mut().retain(|o| o.frame_id != *frame_id));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::envelope::*;

    #[test]
    fn only_a_call_is_closed_by_its_reply() {
        let envelope = |subsystem, opcode| Envelope {
            version: ENVELOPE_VERSION,
            subsystem,
            opcode,
            flags: 0,
            correlation_id: 9,
            payload_kind: PAYLOAD_EMPTY,
        };
        assert_eq!(call_id(&envelope(SUB_RPC, RPC_INVOKE)), Some(9));
        for (subsystem, opcode) in [
            (SUB_RPC, RPC_CANCEL),
            (SUB_STREAM, STREAM_DATA),
            (SUB_STREAM, STREAM_OPEN),
            (SUB_EVENT, EVENT_SUBSCRIBE),
        ] {
            assert_eq!(call_id(&envelope(subsystem, opcode)), None);
        }
    }
}
