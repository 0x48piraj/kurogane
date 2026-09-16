//! Renderer-side event dispatch.
//!
//! Events are delivered only to their addressed subscription and owning frame.

use cef::*;

use crate::debug;
use crate::ipc::envelope::*;
use crate::ipc::renderer_state::state;
use crate::ipc::FrameId;

/// Handle an event message arriving from the browser (renderer-side dispatch).
pub fn handle_event_renderer(frame: &mut Frame, envelope: &Envelope, payload: &[u8]) -> bool {
    let addressed = FrameId::of(frame);
    match envelope.opcode {
        EVENT_EMIT => on_emit(&addressed, envelope, payload),
        EVENT_REFUSED => on_refused(&addressed, envelope, payload),
        _ => {
            debug!("[Event Renderer] unknown opcode {}", envelope.opcode);
            false
        }
    }
}

/// The browser refused a subscription: remove it and call its `onError`
/// with `"{code}: {message}"`, the form the bridge's `toError` parses.
fn on_refused(addressed: &FrameId, envelope: &Envelope, payload: &[u8]) -> bool {
    let (code, message) = decode_error_payload(payload);
    let removed = state().refused(envelope.correlation_id as i32, addressed);
    debug!(
        "[Event Renderer] subscription {} refused: {}",
        envelope.correlation_id, message
    );

    // The lock is released: the callback may call core.on() or core.off().
    let Some((context, Some(on_error))) = removed else {
        return true;
    };
    if context.enter() == 0 {
        return true;
    }
    let text = v8_value_create_string(Some(&CefString::from(
        format!("{code}: {message}").as_str(),
    )));
    on_error.execute_function(None, Some(&[text]));
    context.exit();
    true
}

fn on_emit(addressed: &FrameId, envelope: &Envelope, payload: &[u8]) -> bool {
    let Some((event_name, data)) = decode_cmd_payload(payload) else {
        debug!("[Event Renderer] invalid emit payload");
        return false;
    };

    let target = state().event(envelope.correlation_id as i32, addressed);
    // The lock is released before JavaScript runs: the callback may re-enter
    let Some((context, callback)) = target else {
        debug!(
            "[Event Renderer] '{}' for unknown or foreign subscription {}",
            event_name, envelope.correlation_id
        );
        return true;
    };
    if context.enter() == 0 {
        return true;
    }
    let text = String::from_utf8_lossy(data);
    let value = v8_value_create_string(Some(&CefString::from(text.as_ref())));
    callback.execute_function(None, Some(&[value]));
    context.exit();
    true
}
