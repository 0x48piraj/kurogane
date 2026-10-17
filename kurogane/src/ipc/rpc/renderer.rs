use tanso::*;

use tracing::debug;
use crate::ipc::browser_state::ErrorCode;
use crate::ipc::envelope::*;
use crate::ipc::renderer_outbox;
use crate::ipc::renderer_state::with_state;
use crate::ipc::utils::{array_buffer, rejection};
use crate::ipc::FrameId;

/// Handle an RPC response arriving from the browser (renderer-side dispatch).
///
/// Settles the promise only for the context that asked, in the frame the
/// browser addressed; any other answer is dropped.
pub fn handle_rpc_renderer(
    addressed: &FrameId,
    envelope: &Envelope,
    payload: &[u8],
    filled: Option<V8BackingStore>,
) -> bool {
    if !matches!(envelope.opcode, RPC_RESOLVE | RPC_REJECT) {
        debug!("[RPC Renderer] unknown opcode {}", envelope.opcode);
        return false;
    }
    let id = envelope.correlation_id as i32;
    let settled = with_state(|state| state.settle(id, addressed));
    renderer_outbox::answered(addressed, envelope.correlation_id);
    // The lock is released before JavaScript runs
    let Some((context, promise)) = settled else {
        debug!("[RPC Renderer] answer for unknown or foreign id={}", id);
        return true;
    };
    if context.enter() == 0 {
        debug!("[RPC Renderer] failed to enter V8 context for id={}", id);
        return true;
    }
    settle(&promise, envelope, payload, filled);
    context.exit();
    true
}

/// Settles a run of answers to calls. Answers to one context settle inside
/// one enter and exit, so the page's continuations run together when it
/// exits.
pub(crate) fn settle_answers<'p>(
    addressed: &FrameId,
    answers: impl Iterator<Item = (Envelope, &'p [u8])>,
) {
    let mut entered: Option<V8Context> = None;
    for (envelope, data) in answers {
        let id = envelope.correlation_id as i32;
        let settled = with_state(|state| state.settle(id, addressed));
        renderer_outbox::answered(addressed, envelope.correlation_id);
        let Some((context, promise)) = settled else {
            debug!("[RPC Renderer] answer for unknown or foreign id={}", id);
            continue;
        };
        let same = entered
            .as_ref()
            .is_some_and(|current| current.is_same(Some(&mut context.clone())) != 0);
        if !same {
            if let Some(current) = entered.take() {
                current.exit();
            }
            if context.enter() == 0 {
                debug!("[RPC Renderer] failed to enter V8 context for id={}", id);
                continue;
            }
            entered = Some(context);
        }
        settle(&promise, &envelope, data, None);
    }
    if let Some(current) = entered {
        current.exit();
    }
}

/// Resolves or rejects `promise` with an answer, inside its entered context.
fn settle(promise: &V8Value, envelope: &Envelope, payload: &[u8], filled: Option<V8BackingStore>) {
    match (envelope.opcode, envelope.payload_kind) {
        (RPC_RESOLVE, PAYLOAD_BINARY) => match array_buffer(payload, filled) {
            Some(mut buffer) => {
                promise.resolve_promise(Some(&mut buffer));
            }
            None => {
                let message = rejection(ErrorCode::Buffer.wire(), "Failed to create ArrayBuffer");
                promise.reject_promise(Some(&message));
            }
        },
        (RPC_RESOLVE, _) => {
            let text = String::from_utf8_lossy(payload);
            let mut value = v8_value_create_string(Some(&CefString::from(text.as_ref())));
            promise.resolve_promise(value.as_mut());
        }
        _ => {
            let (code, message) = decode_error_payload(payload);
            let text = rejection(code, &message);
            promise.reject_promise(Some(&text));
        }
    }
}
