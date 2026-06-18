//! IPC message router
//!
//! Central dispatch layer for all IPC messages between browser and renderer.

use cef::*;
use std::sync::Arc;
use crate::ipc::protocol::{get_kind, IpcMsgKind};
use crate::ipc::browser_state::{IpcDispatcher, IpcContext};
use crate::ipc::{rpc, binary};
use crate::debug;

pub fn route_browser(
    frame: &mut Frame,
    args: &ListValue,
    dispatcher: &Arc<IpcDispatcher>,
    ctx: IpcContext,
) -> bool {
    let kind = match get_kind(args) {
        Some(k) => k,
        None => {
            debug!("[IPC Router] invalid ipc message type");
            return false;
        }
    };
    let id = list_get_int(args, 1);
    if id <= 0 {
        debug!("[IPC Router] invalid id {}", id);
        return false;
    }

    debug!("[IPC Browser] message type={:?} id={}", kind, id);

    match kind {
        // JSON invoke
        IpcMsgKind::Invoke => {
            let command = list_get_string(args, 2);
            let payload = list_get_string(args, 3);

            rpc::handle_invoke(frame, id, command, payload, dispatcher, ctx);
        }

        // Binary invoke (inline only; SHM handled in pre-dispatch)
        IpcMsgKind::BinaryInvoke => {
            let command = list_get_string(args, 2);

            // CEF exposes binary via an internal buffer; copy into Vec<u8> to own the data
            if let Some(bin) = args.binary(3) {
                let mut buf = vec![0u8; bin.size()];
                let written = bin.data(Some(&mut buf), 0);
                buf.truncate(written);

                debug!("[IPC Browser] inline binary: {} bytes", written);

                binary::handle_invoke(frame, id, command, &buf, dispatcher, ctx);
                return true;
            }

            debug!("[IPC Router] inline BinaryInvoke missing binary arg");
            binary::send_error(frame, id, "missing binary data".into(), 1);
            return true;
        }

        // Cancel request (renderer to browser)
        IpcMsgKind::CancelRequest => {
            debug!("[IPC Browser] CancelRequest id={}", id);
            dispatcher.cancel_pending(id);
            return true;
        }

        _ => return false,
    }

    true
}

pub fn route_renderer(
    frame: &mut Frame,
    args: &ListValue,
) -> bool {
    let kind = match get_kind(args) {
        Some(k) => k,
        None => {
            debug!("[IPC Router] invalid ipc message type");
            return false;
        }
    };
    let id = list_get_int(args, 1);
    if id <= 0 {
        debug!("[IPC Router] invalid id {}", id);
        return false;
    }

    match kind {
        IpcMsgKind::Resolve => {
            let payload = list_cef_string(args, 2);
            rpc::resolve_cef_string(id, true, &payload, 0);
        }

        IpcMsgKind::Reject => {
            let payload = list_cef_string(args, 2);
            let error_code = args.int(3);
            rpc::resolve_cef_string(id, false, &payload, error_code);
        }

        IpcMsgKind::BinaryResponse => {
            binary::handle_response(frame, id, args);
        }

        _ => return false,
    }

    true
}

//
// Message helpers
//

fn list_get_int(args: &ListValue, idx: usize) -> i32 {
    // binding exposes .int(index)
    args.int(idx)
}

fn list_get_string(args: &ListValue, idx: usize) -> String {
    // binding exposes .string(index) -> CefStringUserfree
    let userfree = args.string(idx);
    // Convert to CefString (borrow conversion) then to Rust String
    let cef: CefString = (&userfree).into();
    cef.to_string()
}

//
// Helpers
//

#[inline(always)]
fn list_cef_string(args: &ListValue, idx: usize) -> CefString {
    (&args.string(idx)).into()
}
