//! RPC (request/response) control center
//!
//! Handles JSON-based request/response pattern with promise correlation.

use cef::*;
use crate::ipc::protocol::{set_kind, IpcMsgKind};
use crate::ipc::renderer_state::{registry};
use crate::ipc::browser_state::{pending_calls, get_dispatcher, IpcResult};
use crate::debug;

// BROWSER
pub fn handle_invoke(
    frame: &mut Frame,
    id: u32,
    command: String,
    payload: String,
) {
    debug!("[RPC Browser] invoke '{}' id={}", command, id);

    let dispatcher = get_dispatcher();

    let result = std::panic::catch_unwind(|| {
        dispatcher.lock().unwrap().dispatch(&command, &payload)
    })
    .unwrap_or_else(|_| Err("IPC handler panicked".to_string()));

    let frame_id = {
        let s: CefString = (&frame.identifier()).into();
        s.to_string()
    };

    pending_calls().lock().unwrap().insert(
        id,
        crate::ipc::browser_state::PendingCall {
            frame: frame.clone(),
            frame_id
        },
    );

    send_response(id, result);
}

/// Send JSON response to renderer
pub fn send_response(id: u32, result: IpcResult) {
    let call = {
        let mut map = pending_calls().lock().unwrap();
        map.remove(&id)
    };

    let Some(call) = call else {
        debug!("[IPC Browser] dropping response {}, caller gone", id);
        return;
    };

    // frame no longer exists
    if call.frame.is_valid() == 0 {
        debug!("[IPC Browser] frame destroyed, dropping {}", id);
        return;
    }

    // navigation changed frame identity
    let current_id = {
        let s: CefString = (&call.frame.identifier()).into();
        s.to_string()
    };

    if current_id != call.frame_id {
        debug!("[IPC Browser] navigation changed frame, dropping stale response {}", id);
        return;
    }

    let mut msg = process_message_create(Some(&CefString::from("ipc"))).unwrap();
    let mut args = msg.argument_list().unwrap();

    match result {
        Ok(payload) => {
            set_kind(&mut args, IpcMsgKind::Resolve);
            args.set_int(1, id as i32);
            args.set_string(2, Some(&CefString::from(payload.as_str())));
        }

        Err(err) => {
            set_kind(&mut args, IpcMsgKind::Reject);
            args.set_int(1, id as i32);
            args.set_string(2, Some(&CefString::from(err.as_str())));
        }
    }

    call.frame.send_process_message(ProcessId::RENDERER, Some(&mut msg));
}

// RENDERER
pub fn resolve_cef_string(id: u32, success: bool, payload: &CefString) {
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
                eprintln!(
                    "[IPC] Failed to enter V8 context for promise id={}",
                    id
                );
                return;
            }

            if success {
                let mut v = v8_value_create_string(Some(payload)).unwrap();
                promise.resolve_promise(Some(&mut v));
            } else {
                promise.reject_promise(Some(payload));
            }

            context.exit(); // microtask checkpoint fires; lock is not held
        }
    }
}
