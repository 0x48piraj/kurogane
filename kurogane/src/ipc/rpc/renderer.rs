use cef::*;

use crate::debug;
use crate::ipc::envelope::*;
use crate::ipc::renderer_state::state;
use crate::ipc::utils::create_array_buffer_from_bytes;
use crate::ipc::FrameId;

/// Handle an RPC response arriving from the browser (renderer-side dispatch).
///
/// Settles the promise only for the context that asked, in the frame the
/// browser addressed; any other answer is dropped.
pub fn handle_rpc_renderer(frame: &mut Frame, envelope: &Envelope, payload: &[u8]) -> bool {
    if !matches!(envelope.opcode, RPC_RESOLVE | RPC_REJECT) {
        debug!("[RPC Renderer] unknown opcode {}", envelope.opcode);
        return false;
    }
    let id = envelope.correlation_id as i32;
    let settled = state().settle(id, &FrameId::of(frame));
    // The lock is released before JavaScript runs
    let Some((context, promise)) = settled else {
        debug!("[RPC Renderer] answer for unknown or foreign id={}", id);
        return true;
    };
    if context.enter() == 0 {
        debug!("[RPC Renderer] failed to enter V8 context for id={}", id);
        return true;
    }
    match (envelope.opcode, envelope.payload_kind) {
        (RPC_RESOLVE, PAYLOAD_BINARY) => match create_array_buffer_from_bytes(payload) {
            Some(mut buffer) => {
                promise.resolve_promise(Some(&mut buffer));
            }
            None => {
                let message = CefString::from("-2: Failed to create ArrayBuffer");
                promise.reject_promise(Some(&message));
            }
        },
        (RPC_RESOLVE, _) => {
            let text = String::from_utf8_lossy(payload);
            let mut value = v8_value_create_string(Some(&CefString::from(text.as_ref())));
            promise.resolve_promise(value.as_mut());
        }
        _ => {
            // The "{code}: {message}" form of IpcError's Display, which the
            // bridge's toError parses
            let (code, message) = decode_error_payload(payload);
            let text = CefString::from(format!("{code}: {message}").as_str());
            promise.reject_promise(Some(&text));
        }
    }
    context.exit();
    true
}
