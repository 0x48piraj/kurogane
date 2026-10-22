//! Renderer-side stream subsystem dispatch.
//!
//! Handles the browser's opened, data, end and error stream messages.
//! Messages reach only streams the addressed frame opened, through the
//! callbacks bound at the open. The browser answers an open before it sends
//! anything else of the stream. A stream still opening receives nothing but
//! that answer.

#![deny(clippy::unwrap_used, clippy::expect_used)]

use tanso::*;

use tracing::debug;
use crate::ipc::envelope::*;
use crate::ipc::renderer_state::with_state;
use crate::ipc::utils::{array_buffer, rejection};
use crate::ipc::FrameId;

/// Handle a stream message arriving from the browser (renderer-side dispatch).
pub fn handle_stream_renderer(
    addressed: &FrameId,
    envelope: &Envelope,
    payload: &[u8],
    filled: Option<V8BackingStore>,
) -> bool {
    let id = envelope.correlation_id as i32;
    match envelope.opcode {
        STREAM_BROWSER_OPENED => on_opened(id, addressed),
        STREAM_BROWSER_DATA => on_data(id, addressed, payload, filled),
        STREAM_BROWSER_END => on_end(id, addressed, payload),
        STREAM_BROWSER_ERROR => on_error(id, addressed, payload),
        _ => {
            debug!("[Stream Renderer] unknown opcode {}", envelope.opcode);
            false
        }
    }
}

/// `STREAM_BROWSER_OPENED` reports that the handler accepted the open. The
/// stream's callbacks become live and `openStream` resolves with its id.
fn on_opened(id: i32, addressed: &FrameId) -> bool {
    let opened = with_state(|state| state.stream_opened(id, addressed));
    let Some((context, promise)) = opened else {
        debug!(
            "[Stream Renderer] opened for unknown or foreign stream {}",
            id
        );
        return true;
    };
    if context.enter() == 0 {
        return true;
    }
    let mut value = v8_value_create_uint(id as u32);
    promise.resolve_promise(value.as_mut());
    context.exit();
    true
}

/// `STREAM_BROWSER_DATA` for an open stream. A chunk for a stream still
/// opening is dropped.
fn on_data(id: i32, addressed: &FrameId, payload: &[u8], filled: Option<V8BackingStore>) -> bool {
    let target = with_state(|state| state.stream_data(id, addressed));
    let Some((context, callback)) = target else {
        debug!(
            "[Stream Renderer] data for unknown or foreign stream {}",
            id
        );
        return true;
    };
    if context.enter() == 0 {
        return true;
    }
    match array_buffer(payload, filled) {
        Some(buffer) => {
            callback.execute_function(None, Some(&[Some(buffer)]));
        }
        None => debug!(
            "[Stream Renderer] failed to create ArrayBuffer for stream {}",
            id
        ),
    }
    context.exit();
    true
}

/// `STREAM_BROWSER_END` ends an open stream. An END for a stream still
/// opening is dropped and leaves its open pending.
fn on_end(id: i32, addressed: &FrameId, payload: &[u8]) -> bool {
    let ended = with_state(|state| state.stream_end(id, addressed));
    let Some((context, callback)) = ended else {
        debug!("[Stream Renderer] end for unknown or foreign stream {}", id);
        return true;
    };
    if context.enter() == 0 {
        return true;
    }
    let text = String::from_utf8_lossy(payload);
    let value = v8_value_create_string(Some(&CefString::from(text.as_ref())));
    callback.execute_function(None, Some(&[value]));
    context.exit();
    true
}

/// `STREAM_BROWSER_ERROR` rejects an open the browser refused, or fails an
/// open stream.
fn on_error(id: i32, addressed: &FrameId, payload: &[u8]) -> bool {
    let (code, message) = decode_error_payload(payload);

    let failed = with_state(|state| state.stream_open_failed(id, addressed));
    if let Some((context, promise)) = failed {
        if context.enter() == 0 {
            return true;
        }
        let text = rejection(code, &message);
        promise.reject_promise(Some(&text));
        context.exit();
        return true;
    }

    let errored = with_state(|state| state.stream_error(id, addressed));
    let Some((context, callback)) = errored else {
        debug!(
            "[Stream Renderer] error for unknown or foreign stream {}",
            id
        );
        return true;
    };
    if context.enter() == 0 {
        return true;
    }
    let value = v8_value_create_string(Some(&CefString::from(message.as_ref())));
    callback.execute_function(None, Some(&[value]));
    context.exit();
    true
}
