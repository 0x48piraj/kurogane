use cef::*;

use crate::debug;
use crate::ipc::envelope::*;
use crate::ipc::renderer_state::registry;

/// Handle a binary response arriving from the browser (renderer-side dispatch).
pub fn handle_binary_renderer(_frame: &mut Frame, envelope: &Envelope, payload: &[u8]) -> bool {
    match envelope.opcode {
        BINARY_RESPONSE => on_response(envelope, payload),
        BINARY_REJECT => on_reject(envelope, payload),
        _ => {
            debug!("[Binary Renderer] unknown opcode {}", envelope.opcode);
            false
        }
    }
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
    debug!("[Binary Renderer] reject id={}, code={}", id, error_code);

    let entry = {
        registry().lock().unwrap().take(id)
    };

    match entry {
        None => {
            eprintln!(
                "[IPC WARNING] binary reject for unknown promise id={} (likely page reload)",
                id
            );
        }
        Some((context, promise, _subsystem)) => {
            if context.enter() == 0 {
                eprintln!("[IPC] failed to enter V8 context for binary promise id={}", id);
                return false;
            }

            let reject_msg = CefString::from(format!("ERR_{}: {}", error_code, error_msg).as_str());
            promise.reject_promise(Some(&reject_msg));

            context.exit();
        }
    }

    true
}

fn on_response(envelope: &Envelope, payload: &[u8]) -> bool {
    let id = envelope.correlation_id as i32;
    debug!("[Binary Renderer] response id={}, {} bytes", id, payload.len());

    let entry = {
        registry().lock().unwrap().take(id)
    };

    match entry {
        None => {
            eprintln!(
                "[IPC WARNING] binary response for unknown promise id={} (likely page reload)",
                id
            );
        }
        Some((context, promise, _subsystem)) => {
            if context.enter() == 0 {
                eprintln!("[IPC] failed to enter V8 context for binary promise id={}", id);
                return false;
            }

            match create_array_buffer_from_bytes(payload) {
                Some(mut buf) => {
                    promise.resolve_promise(Some(&mut buf));
                }
                None => {
                    let reject_msg = CefString::from("ERR_-2: Failed to create ArrayBuffer");
                    promise.reject_promise(Some(&reject_msg));
                }
            }

            context.exit();
        }
    }

    true
}

fn create_array_buffer_from_bytes(payload: &[u8]) -> Option<V8Value> {
    let mut store = v8_backing_store_create(payload.len())?;

    if store.is_valid() == 0 {
        return None;
    }

    unsafe {
        std::ptr::copy_nonoverlapping(
            payload.as_ptr(),
            store.data() as *mut u8,
            payload.len(),
        );
    }

    v8_value_create_array_buffer_from_backing_store(Some(&mut store))
}
