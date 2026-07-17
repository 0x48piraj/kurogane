//! Renderer-side stream subsystem dispatch.
//!
//! Handles STREAM_BROWSER_DATA, STREAM_BROWSER_END, STREAM_BROWSER_ERROR
//! from the browser. Dispatches to V8 callbacks registered via core.onStreamData/End/Error.

use cef::*;

use crate::debug;
use crate::ipc::envelope::*;
use crate::ipc::renderer_state::{registry, stream_callback_registry};
use crate::ipc::browser_state::IpcError;
use crate::ipc::utils::create_array_buffer_from_bytes;

/// Handle a stream message arriving from the browser (renderer-side dispatch).
pub fn handle_stream_renderer(_frame: &mut Frame, envelope: &Envelope, payload: &[u8]) -> bool {
    match envelope.opcode {
        STREAM_BROWSER_DATA => on_data(envelope, payload),
        STREAM_BROWSER_END => on_end(envelope, payload),
        STREAM_BROWSER_ERROR => on_error(envelope, payload),
        _ => {
            debug!("[Stream Renderer] unknown opcode {}", envelope.opcode);
            false
        }
    }
}

fn on_data(envelope: &Envelope, payload: &[u8]) -> bool {
    let stream_id = envelope.correlation_id as i32;

    let entry = {
        let registry = stream_callback_registry().lock().unwrap();
        registry.collect_data(stream_id)
    };

    match entry {
        None => {
            debug!("[Stream Renderer] no data callback for stream_id={}", stream_id);
            true
        }
        Some((context, callback)) => {
            if context.enter() == 0 {
                debug!("[Stream Renderer] failed to enter V8 context for stream_id={}", stream_id);
                return true;
            }

            match create_array_buffer_from_bytes(payload) {
                Some(buf) => {
                    let args: [Option<V8Value>; 1] = [Some(buf)];
                    callback.execute_function(None, Some(&args));
                }
                None => {
                    debug!("[Stream Renderer] failed to create ArrayBuffer for stream_id={}", stream_id);
                }
            }

            context.exit();
            true
        }
    }
}

fn on_end(envelope: &Envelope, payload: &[u8]) -> bool {
    let stream_id = envelope.correlation_id as i32;

    // Resolve the pending open() promise before treating END as a stream completion event
    let entry = registry().lock().unwrap().take(stream_id);
    if let Some((context, promise, _)) = entry {
        if context.enter() == 0 {
            return false;
        }
        let mut stream_v8 = v8_value_create_uint(stream_id as u32).unwrap();
        promise.resolve_promise(Some(&mut stream_v8));
        context.exit();
        return true;
    }

    // No pending open() promise; treat this as a normal stream completion
    let entry = {
        let mut registry = stream_callback_registry().lock().unwrap();
        let cb = registry.take_end(stream_id);
        registry.clear_stream(stream_id);
        cb
    };

    match entry {
        None => {
            debug!("[Stream Renderer] no end callback for stream_id={}", stream_id);
            true
        }
        Some((context, callback)) => {
            if context.enter() == 0 {
                debug!("[Stream Renderer] failed to enter V8 context for stream_id={}", stream_id);
                return true;
            }

            let result_str = String::from_utf8_lossy(payload);
            let payload_v8 = v8_value_create_string(Some(&CefString::from(result_str.as_ref()))).unwrap();
            let args: [Option<V8Value>; 1] = [Some(payload_v8)];
            callback.execute_function(None, Some(&args));

            context.exit();
            true
        }
    }
}

fn on_error(envelope: &Envelope, payload: &[u8]) -> bool {
    let stream_id = envelope.correlation_id as i32;

    let err_str = String::from_utf8_lossy(payload);

    // Check if there's a pending open() promise for this id -> open failed
    let entry = registry().lock().unwrap().take(stream_id);
    if let Some((context, promise, _)) = entry {
        if context.enter() == 0 {
            return false;
        }
        let msg = CefString::from(IpcError::new(err_str.as_ref(), -1).to_string().as_str());
        promise.reject_promise(Some(&msg));
        context.exit();
        return true;
    }

    // Otherwise it's a mid-stream error -> existing onError callback path
    let entry = {
        let mut registry = stream_callback_registry().lock().unwrap();
        let cb = registry.take_error(stream_id);
        registry.clear_stream(stream_id);
        cb
    };

    match entry {
        None => {
            debug!("[Stream Renderer] no error callback for stream_id={}", stream_id);
            true
        }
        Some((context, callback)) => {
            if context.enter() == 0 {
                debug!("[Stream Renderer] failed to enter V8 context for stream_id={}", stream_id);
                return true;
            }

            let payload_v8 = v8_value_create_string(Some(&CefString::from(err_str.as_ref()))).unwrap();
            let args: [Option<V8Value>; 1] = [Some(payload_v8)];
            callback.execute_function(None, Some(&args));

            context.exit();
            true
        }
    }
}
