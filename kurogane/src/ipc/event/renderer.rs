//! Renderer-side event dispatch.
//!
//! Receives event messages from the browser and invokes the registered V8
//! callbacks for the corresponding event name.

use cef::*;

use crate::debug;
use crate::ipc::envelope::*;
use crate::ipc::renderer_state::renderer_state;

/// Handle an event message arriving from the browser (renderer-side dispatch).
pub fn handle_event_renderer(_frame: &mut Frame, envelope: &Envelope, payload: &[u8]) -> bool {
    match envelope.opcode {
        EVENT_EMIT => on_emit(payload),
        _ => {
            debug!("[Event Renderer] unknown opcode {}", envelope.opcode);
            false
        }
    }
}

fn on_emit(payload: &[u8]) -> bool {
    let (event_name, data) = match decode_cmd_payload(payload) {
        Some(v) => v,
        None => {
            debug!("[Event Renderer] invalid emit payload");
            return false;
        }
    };

    let payload_str = String::from_utf8_lossy(data);

    // Collect callbacks under lock, then release lock before JS invocation
    // to prevent reentrant deadlock if a callback calls core.off() or core.on().
    let to_call = {
        let mut state = renderer_state().lock().unwrap();
        state.events.collect_callbacks(event_name)
    };
    let count = to_call.len();
    for (context, callback) in to_call {
        if context.enter() == 0 {
            continue;
        }
        let payload_v8 = v8_value_create_string(Some(&CefString::from(payload_str.as_ref()))).unwrap();
        let args: [Option<V8Value>; 1] = [Some(payload_v8)];
        callback.execute_function(
            None,
            Some(&args),
        );
        context.exit();
    }

    if count == 0 {
        debug!("[Event Renderer] no callbacks for '{}'", event_name);
    }
    true
}
