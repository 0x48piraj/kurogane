//! CEF IPC message browser process entrypoint
//!
//! Boundary between CEF's message system and the IPC infrastructure.

use tanso::*;

use crate::browser_registry::BrowserId;
use tracing::debug;
use crate::acl::Origin;
use crate::ipc::browser_state::{effective_origin, FrameId, IpcContext};
use crate::ipc::batch;
use crate::ipc::browser_outbox::{self, ReplyBatch};
use crate::ipc::envelope::{FLAG_ANSWERS, KNOWN_FLAGS, RPC_BATCH, RPC_INVOKE, SUB_RPC};
use crate::ipc::router::IpcRouter;
use crate::ipc::transport::message::receive_from_renderer;
use crate::runtime::AppHandle;

pub fn handle_ipc_message(
    app: &AppHandle,
    browser: &mut Browser,
    frame: &mut Frame,
    message: &ProcessMessage,
    browser_id: Option<BrowserId>,
) -> bool {
    let name: CefString = (&message.name()).into();
    let name = name.to_string();

    if !name.starts_with("kurogane_") {
        return false;
    }

    let Some(received) = receive_from_renderer(message, app.renderer_sandbox()) else {
        debug!("[IPC Browser] failed to extract message");
        // Calls that cannot be read still close, so the frame's queued calls
        // never wait for them
        if name == "kurogane_rpc" {
            browser_outbox::close_unread(frame);
        }
        return false;
    };
    let envelope = received.envelope();

    // A message of calls that asks for answers is closed by one message of
    // answers, however it ends
    let calls = envelope.subsystem == SUB_RPC
        && matches!(envelope.opcode, RPC_INVOKE | RPC_BATCH)
        && envelope.flags & FLAG_ANSWERS != 0;
    let frame_id = FrameId::of(frame);
    let _answers = calls.then(|| ReplyBatch::begin(frame, frame_id.clone()));

    if envelope.flags & !KNOWN_FLAGS != 0 {
        debug!(
            "[IPC Browser] unknown envelope flags {:#04x}; message dropped",
            envelope.flags
        );
        return true;
    }

    // Not registered yet, or no longer: nothing is dispatched or kept for
    // the browser, and what the message opens is refused, so its sender
    // does not wait forever
    let is_batch = envelope.subsystem == SUB_RPC && envelope.opcode == RPC_BATCH;

    let Some(browser_id) = browser_id else {
        debug!(
            "[IPC Browser] message from unregistered browser cef_id={}",
            browser.identifier()
        );
        if is_batch {
            for (call, _) in batch::items(received.payload()) {
                IpcRouter::refuse_unregistered(frame, &call);
            }
        } else {
            IpcRouter::refuse_unregistered(frame, &envelope);
        }
        return true;
    };

    let frame_url: CefString = (&frame.url()).into();
    let url_origin = Origin::from_url(&frame_url.to_string());

    if is_batch {
        handle_batch(
            app,
            frame,
            frame_id,
            received.payload(),
            browser_id,
            &url_origin,
        );
        return true;
    }

    let ctx = IpcContext {
        browser_id,
        frame: frame_id,
        origin: effective_origin(&url_origin, envelope.flags),
        url_origin,
    };

    app.router()
        .route_browser(app, frame, &envelope, received.payload(), ctx)
}

/// Runs each call of a batch as its own message would run, from one read of
/// the frame's URL. The replies the calls make meanwhile leave as one message.
fn handle_batch(
    app: &AppHandle,
    frame: &mut Frame,
    sender: FrameId,
    payload: &[u8],
    browser_id: BrowserId,
    url_origin: &Origin,
) {
    for (call, data) in batch::items(payload) {
        // A batch carries calls only
        if call.subsystem != SUB_RPC || call.opcode != RPC_INVOKE || call.flags & !KNOWN_FLAGS != 0
        {
            debug!(
                "[IPC Browser] batch item {}:{} dropped",
                call.subsystem, call.opcode
            );
            continue;
        }
        let ctx = IpcContext {
            browser_id,
            frame: sender.clone(),
            origin: effective_origin(url_origin, call.flags),
            url_origin: url_origin.clone(),
        };
        app.router().route_browser(app, frame, &call, data, ctx);
    }
}
