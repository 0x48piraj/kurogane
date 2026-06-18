//! RPC (request/response) control center
//!
//! Handles JSON-based request/response pattern with promise correlation.

use cef::*;
use std::sync::Arc;
use std::panic::{catch_unwind, AssertUnwindSafe};
use crate::ipc::protocol::{set_kind, IpcMsgKind, IpcId};
use crate::ipc::renderer_state::registry;
use crate::ipc::browser_state::{IpcDispatcher, IpcResult, IpcContext};
use crate::debug;

// Browser

pub fn handle_invoke(
    frame: &mut Frame,
    id: IpcId,
    command: String,
    payload: String,
    dispatcher: &Arc<IpcDispatcher>,
    ctx: IpcContext,
) {
    debug!("[RPC Browser] invoke '{}' id={}", command, id);

    let result = catch_unwind(AssertUnwindSafe(|| {
        dispatcher.dispatch_with_context(&command, &payload, ctx)
    }));

    let (response, error_code) = match result {
        Ok(Ok(payload)) => (Ok(payload), 0),
        Ok(Err(msg)) => (Err(msg), 0),
        Err(_) => (Err("IPC handler panicked".to_string()), -1),
    };

    send_response(frame, id, response, error_code);
}

/// Send JSON response to renderer
pub fn send_response(frame: &Frame, id: IpcId, result: IpcResult, error_code: i32) {
    // frame no longer exists
    if frame.is_valid() == 0 {
        debug!("[IPC Browser] frame destroyed, dropping id={}", id);
        return;
    }

    let mut msg = match process_message_create(Some(&CefString::from("ipc"))) {
        Some(m) => m,
        None => {
            debug!("[IPC Browser] failed to create process message");
            return;
        }
    };

    let mut args = match msg.argument_list() {
        Some(a) => a,
        None => {
            debug!("[IPC Browser] missing argument list");
            return;
        }
    };

    match result {
        Ok(payload) => {
            set_kind(&mut args, IpcMsgKind::Resolve);
            args.set_int(1, id);
            args.set_string(2, Some(&CefString::from(payload.as_str())));
        }

        Err(err) => {
            set_kind(&mut args, IpcMsgKind::Reject);
            args.set_int(1, id);
            args.set_string(2, Some(&CefString::from(err.as_str())));
            args.set_int(3, error_code);
        }
    }

    frame.send_process_message(ProcessId::RENDERER, Some(&mut msg));
}

// Renderer

pub fn resolve_cef_string(id: IpcId, success: bool, payload: &CefString, error_code: i32) {
    // Remove entry under lock; drop it before touching V8.
    // Holding the mutex across context.exit() can deadlock due to microtask reentrancy.
    let entry = {
        registry().lock().unwrap().take(id)
    };

    match entry {
        None => {
            eprintln!(
                "[IPC WARNING] response for unknown promise id={} (likely page reload)",
                id
            );
        }

        Some((context, promise)) => {
            if context.enter() == 0 {
                eprintln!("[IPC] failed to enter V8 context for promise id={}", id);
                return;
            }

            if success {
                let mut v = v8_value_create_string(Some(payload)).unwrap();
                promise.resolve_promise(Some(&mut v));
            } else {
                let reject_msg = format!("ERR_{}: {}", error_code, payload);
                let reject_cef = CefString::from(reject_msg.as_str());
                promise.reject_promise(Some(&reject_cef));
            }

            context.exit(); // microtask checkpoint fires; lock is not held
        }
    }
}
