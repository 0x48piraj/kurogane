use cef::*;

use crate::debug;
use crate::ipc::envelope::*;
use crate::ipc::renderer_state::registry;

/// Handle an RPC response arriving from the browser (renderer-side dispatch).
pub fn handle_rpc_renderer(_frame: &mut Frame, envelope: &Envelope, payload: &[u8]) -> bool {
    match envelope.opcode {
        RPC_RESOLVE => on_resolve(envelope, payload),
        RPC_REJECT => on_reject(envelope, payload),
        _ => {
            debug!("[RPC Renderer] unknown opcode {}", envelope.opcode);
            false
        }
    }
}

fn on_resolve(envelope: &Envelope, payload: &[u8]) -> bool {
    let id = envelope.correlation_id as i32;
    let payload_str = String::from_utf8_lossy(payload);
    resolve_cef_string(id, true, &CefString::from(payload_str.as_ref()), 0);
    true
}

fn on_reject(envelope: &Envelope, payload: &[u8]) -> bool {
    let id = envelope.correlation_id as i32;
    let (error_code, error_msg) = if payload.len() >= 4 {
        let code = i32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
        let msg = String::from_utf8_lossy(&payload[4..]);
        (code, msg)
    } else {
        (0, String::from_utf8_lossy(payload))
    };
    resolve_cef_string(id, false, &CefString::from(error_msg.as_ref()), error_code);
    true
}

/// Look up a registered promise by id and resolve or reject it via V8.
pub fn resolve_cef_string(id: i32, success: bool, payload: &CefString, error_code: i32) {
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
        Some((context, promise, _subsystem)) => {
            if context.enter() == 0 {
                eprintln!("[IPC] failed to enter V8 context for promise id={}", id);
                return;
            }

            if success {
                let mut v = v8_value_create_string(Some(payload)).unwrap();
                promise.resolve_promise(Some(&mut v));
            } else {
                let reject_msg = format!("{}: {}", error_code, payload);
                let reject_cef = CefString::from(reject_msg.as_str());
                promise.reject_promise(Some(&reject_cef));
            }

            context.exit();
        }
    }
}
