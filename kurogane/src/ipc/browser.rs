//! CEF IPC message browser process entrypoint
//!
//! Boundary between CEF's message system and the IPC infrastructure.

use cef::*;
use crate::debug;

pub fn handle_ipc_message(
    _browser: &mut Browser,
    frame: &mut Frame,
    message: &mut ProcessMessage,
) -> bool {
    let name: CefString = (&message.name()).into();
    if name.to_string() != "ipc" {
        return false;
    }

    let Some(args) = message.argument_list() else {
        debug!("[IPC Browser] missing argument list");
        return false;
    };

    crate::ipc::router::route_browser(frame, &args)
}
