//! CEF IPC message browser process entrypoint
//!
//! Boundary between CEF's message system and the IPC infrastructure.

use cef::*;

use crate::browser_registry::BrowserId;
use crate::debug;
use crate::acl::Origin;
use crate::ipc::browser_state::{effective_origin, FrameId, IpcContext};
use crate::ipc::envelope::KNOWN_FLAGS;
use crate::ipc::router::IpcRouter;
use crate::ipc::transport::message::extract_message;
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

    let received = match extract_message(message) {
        Some(m) => m,
        None => {
            debug!("[IPC Browser] failed to extract message");
            return false;
        }
    };

    let (envelope, payload) = received.as_envelope_payload();
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
        IpcRouter::refuse_unregistered(frame, &envelope);
        return true;
    };

    let frame_url: CefString = (&frame.url()).into();
    let url_origin = Origin::from_url(&frame_url.to_string());

    let ctx = IpcContext {
        browser_id,
        frame: FrameId::of(frame),
        origin: effective_origin(&url_origin, envelope.flags),
        url_origin,
    };

    app.router()
        .route_browser(app, frame, &envelope, payload, ctx)
}
