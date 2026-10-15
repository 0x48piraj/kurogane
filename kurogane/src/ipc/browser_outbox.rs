//! The browser's messages on their way to a renderer.
//!
//! The replies made while a message of calls is handled leave with the
//! message of answers that closes it. Replies made off the UI thread and
//! answers without replies wait for one task on the UI thread, which sends
//! each frame's as one message. Any other message to a frame sends what waits
//! for it first, so the page sees the browser's order.

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Mutex, OnceLock};

use tanso::*;
use tracing::debug;

use crate::ipc::batch::Batch;
use crate::ipc::envelope::Envelope;
use crate::ipc::transport::message::build_message;
use crate::ipc::FrameId;

/// Replies collected for the frame whose message of calls is being handled.
struct Collected {
    sender: FrameId,
    frame: Frame,
    replies: Batch,
}

thread_local! {
    static COLLECTED: RefCell<Option<Collected>> = const { RefCell::new(None) };
}

/// Collects the replies made while a message of calls from `sender` is
/// handled. On drop it closes that message with them at once. Without any it
/// closes it from the UI thread's next task, beside the replies made off the
/// UI thread meanwhile. A message handled inside another's handling, as in a
/// nested message loop, collects its own, then the outer one's collecting
/// resumes.
pub(crate) struct ReplyBatch {
    /// The collecting this one interrupted.
    outer: Option<Collected>,
}

impl ReplyBatch {
    pub(crate) fn begin(frame: &Frame, sender: FrameId) -> Self {
        let outer = COLLECTED.with(|collected| {
            collected.borrow_mut().replace(Collected {
                sender,
                frame: frame.clone(),
                replies: Batch::default(),
            })
        });
        // The interrupted message's replies leave before this one's
        let outer = outer.map(|mut outer| {
            send_batch(&outer.frame, &std::mem::take(&mut outer.replies));
            outer
        });
        Self { outer }
    }
}

impl Drop for ReplyBatch {
    fn drop(&mut self) {
        let outer = self.outer.take();
        let Some(collected) =
            COLLECTED.with(|collected| std::mem::replace(&mut *collected.borrow_mut(), outer))
        else {
            return;
        };
        if collected.replies.is_empty() {
            later(Later::Close {
                frame: collected.frame,
                frame_id: collected.sender,
            });
            return;
        }
        send_waiting();
        if let Some(mut msg) = collected.replies.build_answers("kurogane_rr", 1) {
            collected
                .frame
                .send_process_message(ProcessId::RENDERER, Some(&mut msg));
        }
    }
}

/// Closes a message of calls from `frame` that could not be read. A message
/// that asked for no answers is closed too, which only lets the frame's
/// queued calls leave sooner.
pub(crate) fn close_unread(frame: &Frame) {
    later(Later::Close {
        frame: frame.clone(),
        frame_id: FrameId::of(frame),
    });
}

/// Returns whether replies are being collected on this thread.
pub(crate) fn collecting() -> bool {
    COLLECTED.with(|collected| collected.borrow().is_some())
}

/// Adds a reply to the batch being collected for `sender`. Returns false
/// when none is or the reply is too large to join it.
pub(crate) fn collect(sender: &FrameId, envelope: &Envelope, data: &[u8]) -> bool {
    let (added, full) = COLLECTED.with(|collected| {
        let mut collected = collected.borrow_mut();
        let Some(c) = collected.as_mut().filter(|c| c.sender == *sender) else {
            return (false, None);
        };
        if !c.replies.push(envelope, &[data]) {
            return (false, None);
        }
        let full = c.replies.is_full();
        (
            true,
            full.then(|| (c.frame.clone(), std::mem::take(&mut c.replies))),
        )
    });
    // A full batch leaves at once and collecting goes on in a new one
    if let Some((frame, replies)) = full {
        send_batch(&frame, &replies);
    }
    added
}

/// Sends `msg` to `frame`, after the replies collected for it and what waits
/// for the UI thread's task.
pub(crate) fn send(frame: &Frame, msg: &mut ProcessMessage) {
    let waiting = COLLECTED.with(|collected| {
        collected
            .borrow()
            .as_ref()
            .is_some_and(|c| !c.replies.is_empty())
    });
    if waiting {
        let sender = FrameId::of(frame);
        let taken = COLLECTED.with(|collected| {
            let mut collected = collected.borrow_mut();
            let collected = collected.as_mut().filter(|c| c.sender == sender)?;
            Some(std::mem::take(&mut collected.replies))
        });
        if let Some(replies) = taken {
            send_batch(frame, &replies);
        }
    }
    send_waiting();
    frame.send_process_message(ProcessId::RENDERER, Some(msg));
}

fn send_batch(frame: &Frame, replies: &Batch) {
    if let Some(mut msg) = replies.build("kurogane_rr", 0) {
        frame.send_process_message(ProcessId::RENDERER, Some(&mut msg));
    }
}

/// Something for a frame that waits for the UI thread's task.
enum Later {
    /// A reply made off the UI thread.
    Reply {
        frame: Frame,
        frame_id: FrameId,
        envelope: Envelope,
        data: Vec<u8>,
    },
    /// The close of a message of calls whose handlers made no reply yet.
    Close { frame: Frame, frame_id: FrameId },
}

impl Later {
    fn frame(&self) -> (&Frame, &FrameId) {
        match self {
            Later::Reply {
                frame, frame_id, ..
            }
            | Later::Close { frame, frame_id } => (frame, frame_id),
        }
    }
}

/// What waits for the UI thread's task.
struct Laters {
    send: Sender<Later>,
    /// Read on the UI thread alone.
    receive: Mutex<Receiver<Later>>,
    /// Whether a task that sends them is posted. The task clears it before
    /// it reads the channel, so anything sent meanwhile posts another.
    posted: AtomicBool,
}

fn laters() -> &'static Laters {
    static LATERS: OnceLock<Laters> = OnceLock::new();
    LATERS.get_or_init(|| {
        let (send, receive) = channel();
        Laters {
            send,
            receive: Mutex::new(receive),
            posted: AtomicBool::new(false),
        }
    })
}

fn later(later: Later) {
    let laters = laters();
    if laters.send.send(later).is_err() {
        return;
    }
    if !laters.posted.swap(true, Ordering::SeqCst) {
        let mut task = LaterTask::new();
        post_task(ThreadId::UI, Some(&mut task));
    }
}

/// Sends a reply made off the UI thread from the UI thread's next task,
/// beside the other replies made there.
pub(crate) fn send_later(frame: Frame, frame_id: FrameId, envelope: Envelope, data: Vec<u8>) {
    later(Later::Reply {
        frame,
        frame_id,
        envelope,
        data,
    });
}

/// Sends what waits for the UI thread's task now, when on the UI thread.
fn send_waiting() {
    if laters().posted.load(Ordering::SeqCst) && currently_on(ThreadId::UI) != 0 {
        send_laters();
    }
}

/// Sends everything that waits, each frame's in order as few messages.
fn send_laters() {
    let laters = laters();
    laters.posted.store(false, Ordering::SeqCst);
    let mut waiting: Vec<Later> = match laters.receive.lock() {
        Ok(receive) => receive.try_iter().collect(),
        Err(_) => return,
    };
    // One reply alone goes as itself
    if let [Later::Reply { .. }] = waiting.as_slice()
        && let Some(Later::Reply {
            frame,
            envelope,
            data,
            ..
        }) = waiting.pop()
    {
        if let Some(mut msg) = build_message("kurogane_rr", &envelope, &data) {
            frame.send_process_message(ProcessId::RENDERER, Some(&mut msg));
        }
        return;
    }
    let mut frames: Vec<(FrameId, Frame, Batch, u32)> = Vec::new();
    for later in waiting {
        let (frame, frame_id) = later.frame();
        let index = match frames.iter().position(|(id, ..)| id == frame_id) {
            Some(index) => index,
            None => {
                frames.push((frame_id.clone(), frame.clone(), Batch::default(), 0));
                frames.len() - 1
            }
        };
        let (_, frame, replies, closes) = &mut frames[index];
        match later {
            Later::Close { .. } => *closes += 1,
            Later::Reply { envelope, data, .. } => {
                if !replies.push(&envelope, &[&data]) {
                    // A reply too large for a batch leaves after the ones before it
                    send_batch(frame, &std::mem::take(replies));
                    if let Some(mut msg) = build_message("kurogane_rr", &envelope, &data) {
                        frame.send_process_message(ProcessId::RENDERER, Some(&mut msg));
                    }
                } else if replies.is_full() {
                    send_batch(frame, &std::mem::take(replies));
                }
            }
        }
    }
    for (_, frame, replies, closes) in frames {
        if closes == 0 {
            send_batch(&frame, &replies);
        } else if let Some(mut msg) = replies.build_answers("kurogane_rr", closes) {
            frame.send_process_message(ProcessId::RENDERER, Some(&mut msg));
        }
    }
}

wrap_task! {
    struct LaterTask {}

    impl Task {
        fn execute(&self) {
            // A panic must not unwind across this CEF callback
            if std::panic::catch_unwind(send_laters).is_err() {
                debug!("[IPC Browser] sending waiting replies panicked");
            }
        }
    }
}
