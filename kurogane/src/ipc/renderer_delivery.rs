//! Delivers the browser's messages to the renderer's frames.
//!
//! A large ArrayBuffer for the page is filled on a thread of its own, which
//! keeps the main thread free during big transfers. `renderer_inbox` keeps
//! each frame's order.

use std::cell::RefCell;
use std::sync::mpsc::{SyncSender, sync_channel};
use std::sync::OnceLock;

use tanso::*;
use tracing::debug;

use crate::ipc::envelope::*;
use crate::ipc::renderer_inbox::{Arrival, FILL_MIN, Inbox, Job, MAX_FILLS};
use crate::ipc::router::route_renderer;
use crate::ipc::transport::message::ReceivedMessage;
use crate::ipc::FrameId;

thread_local! {
    /// The inbox of the main thread, which receives every message.
    static INBOX: RefCell<Inbox<Frame, ReceivedMessage, V8BackingStore>> =
        RefCell::new(Inbox::new(post_delivery));
}

fn with_inbox<R>(f: impl FnOnce(&mut Inbox<Frame, ReceivedMessage, V8BackingStore>) -> R) -> R {
    INBOX.with(|inbox| f(&mut inbox.borrow_mut()))
}

/// Routes a message from the browser after the fills that finished, or
/// queues it behind its frame's waiting messages.
pub(crate) fn receive(frame: &mut Frame, message: ReceivedMessage) {
    // A panic must not unwind across this CEF callback
    let received =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| arrive(frame, message)));
    if received.is_err() {
        debug!("[IPC Renderer] delivery panicked; message dropped");
    }
}

fn arrive(frame: &mut Frame, message: ReceivedMessage) {
    deliver_ready();

    // A payload under the fill size holds no ArrayBuffer that fills, which
    // spares small messages the decode
    let len = if message.payload().len() < FILL_MIN {
        0
    } else {
        array_buffer_bytes(&message).map_or(0, <[u8]>::len)
    };
    let arrival = with_inbox(|inbox| {
        inbox.arrive(
            || (FrameId::of(frame), frame.clone()),
            message,
            len,
            |len| v8_backing_store_create(len).filter(|store| store.is_valid() != 0),
        )
    });
    match arrival {
        Arrival::Route(message) => route(frame, &message, None),
        Arrival::Waits => {}
        // A refused job hands its message back to be copied here
        Arrival::Fill(job) => {
            if let Err(refused) = filler().try_send(job) {
                drop(refused);
                deliver_ready();
            }
        }
    }
}

/// Returns the bytes of a message that become an ArrayBuffer for the page.
fn array_buffer_bytes(message: &ReceivedMessage) -> Option<&[u8]> {
    let envelope = message.envelope();
    let payload = message.payload();
    match (envelope.subsystem, envelope.opcode, envelope.payload_kind) {
        (SUB_RPC, RPC_RESOLVE, PAYLOAD_BINARY) => Some(payload),
        (SUB_STREAM, STREAM_BROWSER_DATA, _) => Some(payload),
        (SUB_EVENT, EVENT_EMIT, PAYLOAD_BINARY) => {
            decode_cmd_payload(payload).map(|(_, data)| data)
        }
        _ => None,
    }
}

/// Returns the filler thread's queue, starting the thread on first use.
fn filler() -> &'static SyncSender<Job<ReceivedMessage, V8BackingStore>> {
    static FILLER: OnceLock<SyncSender<Job<ReceivedMessage, V8BackingStore>>> = OnceLock::new();
    FILLER.get_or_init(|| {
        // As many jobs as the cap lets wait; a job refused past it is copied
        // on the main thread
        let (sender, jobs) = sync_channel::<Job<ReceivedMessage, V8BackingStore>>(MAX_FILLS);
        // A thread that does not start drops `jobs`, which refuses every fill
        let started = std::thread::Builder::new()
            .name("kurogane-fill".into())
            .spawn(move || {
                for job in jobs {
                    job.run(|message, store| {
                        array_buffer_bytes(message).is_some_and(|data| store.write(data))
                    });
                }
            });
        if let Err(e) = started {
            debug!("[IPC Renderer] no fill thread: {}", e);
        }
        sender
    })
}

/// Routes every frame's front message while it is ready.
fn deliver_ready() {
    while let Some((mut frame, message, filled)) = with_inbox(Inbox::next) {
        route(&mut frame, &message, filled);
    }
}

fn route(frame: &mut Frame, message: &ReceivedMessage, filled: Option<V8BackingStore>) {
    // A panic must not unwind across this CEF callback
    let routed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        route_renderer(frame, &message.envelope(), message.payload(), filled);
    }));
    if routed.is_err() {
        debug!("[IPC Renderer] dispatch panicked; message dropped");
    }
}

/// Posts a delivery to the main thread, for a fill whose next message may
/// never come.
fn post_delivery() {
    let mut task = DeliverTask::new();
    post_task(ThreadId::RENDERER, Some(&mut task));
}

wrap_task! {
    struct DeliverTask {}

    impl Task {
        fn execute(&self) {
            // A panic must not unwind across this CEF callback
            let delivered = std::panic::catch_unwind(deliver_ready);
            if delivered.is_err() {
                debug!("[IPC Renderer] delivery panicked");
            }
        }
    }
}
