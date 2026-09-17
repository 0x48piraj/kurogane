//! CEF IPC message browser process entrypoint
//!
//! Boundary between CEF's message system and the IPC infrastructure.

use std::sync::Arc;

use cef::*;

use crate::browser_registry::BrowserId;
use crate::debug;
use crate::acl::Origin;
use crate::ipc::browser_state::{effective_origin, FrameId, IpcContext};
use crate::ipc::envelope::KNOWN_FLAGS;
use crate::ipc::transport::message::extract_message;
use crate::ipc::router::IpcRouter;

pub fn handle_ipc_message(
    _browser: &mut Browser,
    frame: &mut Frame,
    message: &ProcessMessage,
    router: &Arc<IpcRouter>,
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

    let frame_url: CefString = (&frame.url()).into();
    let url_origin = Origin::from_url(&frame_url.to_string());

    let ctx = IpcContext {
        browser_id,
        frame: FrameId::of(frame),
        origin: effective_origin(&url_origin, envelope.flags),
        url_origin,
    };

    router.route_browser(frame, &envelope, payload, ctx)
}
