//! CEF IPC message browser process entrypoint
//!
//! Boundary between CEF's message system and the IPC infrastructure.

use tanso::*;

use crate::browser_registry::BrowserId;
use tracing::debug;
use crate::ipc::batch;
use crate::ipc::browser_outbox::{self, Scope};
use crate::ipc::browser_state::{FrameId, IpcContext};
use crate::ipc::envelope::{Envelope, BATCH_ITEMS, FLAG_ANSWERS, KNOWN_FLAGS, SUB_BATCH};
use crate::ipc::router::IpcRouter;
use crate::ipc::transport::message::{is_ipc_message, receive_from_renderer};
use crate::runtime::AppHandle;

pub fn handle_ipc_message(
    app: &AppHandle,
    browser: &mut Browser,
    frame: &mut Frame,
    message: &ProcessMessage,
    browser_id: Option<BrowserId>,
) -> bool {
    if !is_ipc_message(message) {
        return false;
    }

    let frame_id = FrameId::of(frame);
    let Some(received) = receive_from_renderer(message, app.renderer_sandbox()) else {
        debug!("[IPC Browser] failed to extract message");
        // One the page cannot know was lost still closes, so its frame's
        // queued messages never wait for it
        browser_outbox::close_unread(frame, frame_id);
        return true;
    };
    let envelope = received.envelope();

    // Everything sent while the message is handled leaves together, with the
    // answer that closes it when it asked for one, however it ends
    let _scope = Scope::begin(frame, frame_id.clone(), envelope.flags & FLAG_ANSWERS != 0);

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
    let Some(browser_id) = browser_id else {
        debug!(
            "[IPC Browser] message from unregistered browser cef_id={}",
            browser.identifier()
        );
        for (item, _) in messages(&envelope, received.payload()) {
            IpcRouter::refuse_unregistered(frame, &item);
        }
        return true;
    };

    let Some(document) = app.router().document(frame, &frame_id, browser_id) else {
        debug!("[IPC Browser] message from a frame that has gone");
        return true;
    };

    let mut routed = true;
    for (item, payload) in messages(&envelope, received.payload()) {
        let ctx = IpcContext::new(document.clone(), item.flags);
        routed = app.router().route_browser(app, frame, &item, payload, ctx);
    }
    routed
}

/// Returns the messages `envelope` carries, itself or a batch's items in
/// order. An item is a message of its own, never a batch, that asks for no
/// answers; any other item is dropped.
fn messages<'p>(
    envelope: &Envelope,
    payload: &'p [u8],
) -> impl Iterator<Item = (Envelope, &'p [u8])> {
    let is_batch = envelope.subsystem == SUB_BATCH;
    let single = (!is_batch).then_some((*envelope, payload));
    let items = (is_batch && envelope.opcode == BATCH_ITEMS)
        .then(|| batch::items(payload))
        .into_iter()
        .flatten()
        .filter(|(item, _)| {
            let valid =
                item.subsystem != SUB_BATCH && item.flags & (!KNOWN_FLAGS | FLAG_ANSWERS) == 0;
            if !valid {
                debug!(
                    "[IPC Browser] batch item {}:{} flags {:#04x} dropped",
                    item.subsystem, item.opcode, item.flags
                );
            }
            valid
        });
    single.into_iter().chain(items)
}
