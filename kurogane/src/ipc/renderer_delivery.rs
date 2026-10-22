//! Delivers the browser's messages to the renderer's frames.
//!
//! A large ArrayBuffer for the page is filled on a thread of its own, which
//! keeps the main thread free during big transfers. A batch is delivered in
//! slices, each in a task of its own once the first ran. The page's own
//! tasks run between them. A full batch starts in a task of its own.
//! `renderer_inbox` keeps each frame's order. A batch that closes the page's
//! messages in flight lets the frame's queued messages leave
//! (`renderer_outbox`).

#![deny(clippy::unwrap_used, clippy::expect_used)]

use std::cell::{Cell, RefCell};
use std::sync::mpsc::{SyncSender, sync_channel};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use tanso::*;
use tracing::debug;

use crate::ipc::batch::BATCH_MAX;
use crate::ipc::envelope::*;
use crate::ipc::renderer_inbox::{Arrival, FILL_MIN, Inbox, Job, MAX_FILLS};
use crate::ipc::renderer_outbox;
use crate::ipc::router::{route_batch, route_renderer};
use crate::ipc::transport::message::ReceivedMessage;
use crate::ipc::FrameId;

/// Longest a task delivers one batch's messages before the rest waits for a
/// task of its own. It bounds how long a burst holds the page.
const SLICE: Duration = Duration::from_micros(500);

/// Message from the browser delivered from byte `from` of its payload.
struct Arrived {
    message: ReceivedMessage,
    from: usize,
}

thread_local! {
    /// The inbox of the main thread, which receives every message.
    static INBOX: RefCell<Inbox<Frame, Arrived, V8BackingStore>> =
        RefCell::new(Inbox::new(post_delivery));
    /// Whether a task that delivers a full batch is posted.
    static DEFERRAL_POSTED: Cell<bool> = const { Cell::new(false) };
}

fn with_inbox<R>(f: impl FnOnce(&mut Inbox<Frame, Arrived, V8BackingStore>) -> R) -> R {
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
    // A full batch is a burst's, run in a task of its own after the page's
    // tasks queued meanwhile
    let full = message.envelope().subsystem == SUB_BATCH && message.payload().len() >= BATCH_MAX;
    let arrival = with_inbox(|inbox| {
        inbox.arrive(
            || (FrameId::of(frame), frame.clone()),
            Arrived { message, from: 0 },
            len,
            |len| v8_backing_store_create(len).filter(|store| store.is_valid() != 0),
            full,
        )
    });
    match arrival {
        Arrival::Route(arrived) => {
            if let Some(rest) = route(frame, arrived, None) {
                with_inbox(|inbox| inbox.resume(FrameId::of(frame), frame.clone(), rest));
                post_deferral();
            }
        }
        Arrival::Waits => {}
        Arrival::Defer => post_deferral(),
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
fn filler() -> &'static SyncSender<Job<Arrived, V8BackingStore>> {
    static FILLER: OnceLock<SyncSender<Job<Arrived, V8BackingStore>>> = OnceLock::new();
    FILLER.get_or_init(|| {
        // As many jobs as the cap lets wait; a job refused past it is copied
        // on the main thread
        let (sender, jobs) = sync_channel::<Job<Arrived, V8BackingStore>>(MAX_FILLS);
        // A thread that does not start drops `jobs`, which refuses every fill
        let started = std::thread::Builder::new()
            .name("kurogane-fill".into())
            .spawn(move || {
                for job in jobs {
                    job.run(|arrived, store| {
                        array_buffer_bytes(&arrived.message).is_some_and(|data| store.write(data))
                    });
                }
            });
        if let Err(e) = started {
            debug!("[IPC Renderer] no fill thread: {}", e);
        }
        sender
    })
}

/// Routes every frame's front message while it is ready, then posts the
/// task for a full batch or the rest of one that may now be in front.
fn deliver_ready() {
    while let Some((mut frame, arrived, filled, _)) = with_inbox(|inbox| inbox.next(false)) {
        if let Some(rest) = route(&mut frame, arrived, filled) {
            with_inbox(|inbox| inbox.resume(FrameId::of(&frame), frame, rest));
        }
    }
    if with_inbox(|inbox| inbox.has_deferred()) {
        post_deferral();
    }
}

/// Routes one full batch and the messages ready behind it, then posts the
/// next full batch's task.
fn deliver_deferred() {
    let mut deferred = true;
    while let Some((mut frame, arrived, filled, was_deferred)) =
        with_inbox(|inbox| inbox.next(deferred))
    {
        if let Some(rest) = route(&mut frame, arrived, filled) {
            with_inbox(|inbox| inbox.resume(FrameId::of(&frame), frame, rest));
        }
        deferred &= !was_deferred;
    }
    // The next full batch's task; one behind a fill waits for the fill's
    // wake-up, which delivers what is ready and posts it
    if !deferred && with_inbox(|inbox| inbox.has_deferred()) {
        post_deferral();
    }
}

/// Routes a message. Returns the rest of a batch whose slice ran out.
fn route(frame: &mut Frame, arrived: Arrived, filled: Option<V8BackingStore>) -> Option<Arrived> {
    let envelope = arrived.message.envelope();
    let batch = envelope.subsystem == SUB_BATCH;
    // A batch closes the frame's oldest messages, as many as it counts, as
    // its delivery starts
    let closes = if batch && arrived.from == 0 {
        envelope.correlation_id
    } else {
        0
    };
    let addressed = FrameId::of(frame);
    let routed = renderer_outbox::delivering(&addressed, closes, || {
        // A panic must not unwind across this CEF callback
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let payload = arrived.message.payload();
            if batch {
                route_batch(
                    &addressed,
                    payload,
                    arrived.from,
                    Some(Instant::now() + SLICE),
                )
            } else {
                route_renderer(&addressed, &envelope, payload, filled);
                None
            }
        }))
    });
    match routed {
        Ok(rest) => rest.map(|from| Arrived { from, ..arrived }),
        Err(_) => {
            debug!("[IPC Renderer] dispatch panicked; message dropped");
            None
        }
    }
}

/// Posts the task that delivers the next full batch, unless one is posted.
/// One is posted at a time. The page's tasks queued meanwhile run first.
fn post_deferral() {
    if !DEFERRAL_POSTED.replace(true) {
        let mut task = DeferralTask::new();
        post_task(ThreadId::RENDERER, Some(&mut task));
    }
}

wrap_task! {
    struct DeferralTask {}

    impl Task {
        fn execute(&self) {
            DEFERRAL_POSTED.set(false);
            // A panic must not unwind across this CEF callback
            if std::panic::catch_unwind(deliver_deferred).is_err() {
                debug!("[IPC Renderer] delivery panicked");
            }
        }
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
