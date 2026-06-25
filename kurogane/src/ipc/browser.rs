//! CEF IPC message browser process entrypoint
//!
//! Boundary between CEF's message system and the IPC infrastructure.

use std::sync::Arc;

use cef::*;

use crate::browser_registry::BrowserId;
use crate::debug;
use crate::ipc::browser_state::IpcContext;
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

    let ctx = IpcContext {
        browser_id,
        frame_id: None,
    };

    router.route_browser(frame, received.as_bytes(), ctx)
}
