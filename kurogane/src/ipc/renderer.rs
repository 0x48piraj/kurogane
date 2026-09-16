//! V8 bridge for IPC
//!
//! Connects JavaScript to the browser process via CEF messages.
//! Defines the boundary between JavaScript and the native IPC system.

use cef::*;
use std::sync::Arc;
use crate::app::ClientAppRendererDelegate;
use crate::debug;
use crate::ipc::envelope::*;
use crate::ipc::transport::message::{build_message, build_message_parts, extract_message};
use crate::ipc::router;
use crate::ipc::renderer_registry::StreamSink;
use crate::ipc::renderer_state::state;
use crate::ipc::FrameId;
use crate::bridge;

//
// Helpers
//

#[inline(always)]
fn v8_to_string(v: &V8Value) -> String {
    let s: CefString = (&v.string_value()).into();
    s.to_string()
}

// Memory pinning helper
#[inline(always)]
fn with_array_buffer<R>(ptr: *const u8, len: usize, f: impl FnOnce(&[u8]) -> R) -> R {
    // SAFETY:
    //
    // ptr originates from V8 ArrayBuffer backing store.
    //
    // This is safe because:
    //
    // 1. V8 guarantees the backing store is valid for the duration of
    //    this callback (inside a V8 handler).
    //
    // 2. The slice is only exposed through the closure f, preventing it
    //    from escaping this function (imposed by Rust lifetimes).
    //
    // 3. All uses must be synchronous. The data MUST NOT:
    //    - be stored
    //    - be sent across threads
    //    - outlive this function
    //
    // After this function returns, V8 may move or free ArrayBuffer memory.
    // Any use beyond this scope is undefined behavior.
    let slice = unsafe { std::slice::from_raw_parts(ptr, len) };

    f(slice)
}

/// Encode [cmd_len:u16 LE][cmd_bytes] into a Vec.
///
/// Returns None when the command exceeds [`MAX_CMD_LEN`]; see
/// [`encode_cmd_payload`] for why this is not an `as` cast.
fn encode_cmd_header(cmd: &str) -> Option<Vec<u8>> {
    let cmd_bytes = cmd.as_bytes();
    if cmd_bytes.len() > MAX_CMD_LEN {
        return None;
    }

    let mut v = Vec::with_capacity(2 + cmd_bytes.len());
    // Narrowing is safe; bounded by MAX_CMD_LEN on the line above
    let cmd_len = cmd_bytes.len() as u16;
    v.extend_from_slice(&cmd_len.to_le_bytes());
    v.extend_from_slice(cmd_bytes);
    Some(v)
}

//
// Renderer process handler
//

wrap_render_process_handler! {
    pub struct IpcRenderProcessHandler {
        delegates: Vec<Arc<dyn ClientAppRendererDelegate>>,
    }

    impl RenderProcessHandler {
        fn on_web_kit_initialized(&self) {
            for delegate in &self.delegates {
                delegate.on_web_kit_initialized();
            }
        }

        fn on_browser_created(
            &self,
            browser: Option<&mut Browser>,
            extra_info: Option<&mut DictionaryValue>,
        ) {
            let browser_ref = browser.as_deref();
            let extra_info_ref = extra_info.as_deref();

            for delegate in &self.delegates {
                delegate.on_browser_created(browser_ref, extra_info_ref);
            }
        }

        fn on_browser_destroyed(
            &self,
            browser: Option<&mut Browser>,
        ) {
            let browser_ref = browser.as_deref();

            for delegate in &self.delegates {
                delegate.on_browser_destroyed(browser_ref);
            }
        }

        fn on_context_created(
            &self,
            browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            context: Option<&mut V8Context>,
        ) {
            let (Some(context), Some(frame)) = (context, frame) else {
                return;
            };
            let Some(global) = context.global() else {
                return;
            };
            let Some(mut core) = v8_value_create_object(None, None) else {
                return;
            };

            // Read before any page script runs: the document's own view of
            // its origin. A sandboxed document's (iframe `sandbox`, CSP
            // `sandbox`) is "null", whatever its URL; its messages then act
            // for the opaque origin. Unreadable counts as opaque.
            let opaque = !matches!(
                global.value_bykey(Some(&CefString::from("origin"))),
                Some(origin) if origin.is_string() != 0 && v8_to_string(&origin) != "null"
            );
            state().context_created(context.clone(), FrameId::of(frame), opaque);

            // Invoke handler (accepts string or ArrayBuffer payload)
            let mut handler = IpcInvokeHandler::new();
            let mut invoke = v8_value_create_function(
                Some(&CefString::from("invoke")),
                Some(&mut handler),
            ).unwrap();

            core.set_value_bykey(
                Some(&CefString::from("invoke")),
                Some(&mut invoke),
                V8Propertyattribute::default(),
            );

            // Cancel pending promise
            let mut cancel_handler = IpcCancelHandler::new();
            let mut cancel = v8_value_create_function(
                Some(&CefString::from("cancel")),
                Some(&mut cancel_handler),
            ).unwrap();

            core.set_value_bykey(
                Some(&CefString::from("cancel")),
                Some(&mut cancel),
                V8Propertyattribute::default(),
            );

            // Event subscription (on/off)
            let mut on_handler = IpcOnHandler::new();
            let mut on = v8_value_create_function(
                Some(&CefString::from("on")),
                Some(&mut on_handler),
            ).unwrap();

            core.set_value_bykey(
                Some(&CefString::from("on")),
                Some(&mut on),
                V8Propertyattribute::default(),
            );

            let mut off_handler = IpcOffHandler::new();
            let mut off = v8_value_create_function(
                Some(&CefString::from("off")),
                Some(&mut off_handler),
            ).unwrap();

            core.set_value_bykey(
                Some(&CefString::from("off")),
                Some(&mut off),
                V8Propertyattribute::default(),
            );

            // Stream handlers
            let mut open_stream_handler = IpcOpenStreamHandler::new();
            let mut open_stream = v8_value_create_function(
                Some(&CefString::from("openStream")),
                Some(&mut open_stream_handler),
            ).unwrap();

            core.set_value_bykey(
                Some(&CefString::from("openStream")),
                Some(&mut open_stream),
                V8Propertyattribute::default(),
            );

            let mut write_stream_handler = IpcWriteStreamHandler::new();
            let mut write_stream = v8_value_create_function(
                Some(&CefString::from("writeStream")),
                Some(&mut write_stream_handler),
            ).unwrap();

            core.set_value_bykey(
                Some(&CefString::from("writeStream")),
                Some(&mut write_stream),
                V8Propertyattribute::default(),
            );

            let mut end_stream_handler = IpcEndStreamHandler::new();
            let mut end_stream = v8_value_create_function(
                Some(&CefString::from("endStream")),
                Some(&mut end_stream_handler),
            ).unwrap();

            core.set_value_bykey(
                Some(&CefString::from("endStream")),
                Some(&mut end_stream),
                V8Propertyattribute::default(),
            );

            global.set_value_bykey(
                Some(&CefString::from("core")),
                Some(&mut core),
                V8Propertyattribute::default(),
            );

            frame.execute_java_script(
                Some(&CefString::from(bridge::KUROGANE_BRIDGE)),
                None,
                0,
            );

            debug!("[IPC Renderer] Injected window.core.* + kurogane bridge");

            let browser_ref = browser.as_deref();
            for delegate in &self.delegates {
                delegate.on_context_created(browser_ref, Some(frame), Some(context));
            }
        }

        fn on_context_released(
            &self,
            browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            context: Option<&mut V8Context>,
        ) {
            let context_ref = context.as_deref();
            let browser_ref = browser.as_deref();
            let frame_ref = frame.as_deref();

            for delegate in &self.delegates {
                delegate.on_context_released(browser_ref, frame_ref, context_ref);
            }

            if let Some(ctx) = context {
                state().context_released(ctx);
            }
        }

        fn on_uncaught_exception(
            &self,
            browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            context: Option<&mut V8Context>,
            exception: Option<&mut V8Exception>,
            stack_trace: Option<&mut V8StackTrace>,
        ) {
            let browser_ref = browser.as_deref();
            let frame_ref = frame.as_deref();
            let context_ref = context.as_deref();
            let exception_ref = exception.as_deref();
            let stack_trace_ref = stack_trace.as_deref();

            for delegate in &self.delegates {
                delegate.on_uncaught_exception(
                    browser_ref, frame_ref, context_ref, exception_ref, stack_trace_ref,
                );
            }

            if let Some(ex) = exception {
                let msg: CefString = (&ex.message()).into();
                let src: CefString = (&ex.script_resource_name()).into();
                let line = ex.line_number();
                debug!("[Renderer] Uncaught exception at {}:{} - {}", src.to_string(), line, msg.to_string());
            }
        }

        fn on_focused_node_changed(
            &self,
            browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            node: Option<&mut Domnode>,
        ) {
            if node.is_none() {
                let browser_ref = browser.as_deref();
                let frame_ref = frame.as_deref();

                for delegate in &self.delegates {
                    delegate.on_focused_node_changed(browser_ref, frame_ref, None);
                }
                return;
            }

            let node_data = node.as_ref().unwrap();
            let is_editable = node_data.is_editable() != 0;
            let is_form = node_data.is_form_control_element() != 0;

            debug!(
                "[Renderer] Focused node changed: type={} editable={} form={} form_type={}",
                node_data.get_type().get_raw(),
                is_editable,
                is_form,
                is_form
                    .then(|| format!("{:?}", node_data.form_control_element_type()))
                    .as_deref()
                    .unwrap_or("-"),
            );

            let browser_ref = browser.as_deref();
            let frame_ref = frame.as_deref();
            let node_ref = Some(&**node_data);

            for delegate in &self.delegates {
                delegate.on_focused_node_changed(browser_ref, frame_ref, node_ref);
            }
        }

        fn on_process_message_received(
            &self,
            browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            source_process: ProcessId,
            message: Option<&mut ProcessMessage>,
        ) -> i32 {
            {
                let browser_ref = browser.as_deref();
                let frame_ref = frame.as_deref();
                let message_ref = message.as_deref();

                for delegate in &self.delegates {
                    if delegate.on_process_message_received(
                        browser_ref, frame_ref, source_process, message_ref,
                    ) != 0 {
                        return 1;
                    }
                }
            }

            if source_process != ProcessId::BROWSER { return 0; }
            let msg = message.unwrap();

            let name: CefString = (&msg.name()).into();
            if !name.to_string().starts_with("kurogane_") { return 0; }

            let Some(frame) = frame else {
                debug!("[IPC Renderer] missing frame");
                return 0;
            };

            let received = match extract_message(msg) {
                Some(m) => m,
                None => {
                    debug!("[IPC Renderer] failed to extract message");
                    return 1;
                }
            };

            let (envelope, payload) = received.as_envelope_payload();
            // A panic must not unwind across this CEF callback
            let routed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                router::route_renderer(frame, &envelope, payload);
            }));
            if routed.is_err() {
                debug!("[IPC Renderer] dispatch panicked; message dropped");
            }
            1
        }

        fn load_handler(&self) -> Option<LoadHandler> {
            for delegate in &self.delegates {
                if let Some(handler) = delegate.load_handler() {
                    return Some(handler);
                }
            }
            None
        }
    }
}

//
// Invoke handler (accepts string or ArrayBuffer payload)
//

wrap_v8_handler! {
    pub struct IpcInvokeHandler;

    impl V8Handler {
        fn execute(
            &self,
            _name: Option<&CefString>,
            _object: Option<&mut V8Value>,
            arguments: Option<&[Option<V8Value>]>,
            retval: Option<&mut Option<V8Value>>,
            exception: Option<&mut CefString>,
        ) -> i32 {
            // args must be present
            let args = match arguments {
                Some(a) if !a.is_empty() => a,
                _ => {
                    if let Some(exc) = exception {
                        *exc = CefString::from("invoke requires at least a command argument");
                    }
                    return 0;
                }
            };

            // first arg: command string
            let cmd = match args.first() {
                Some(Some(v)) if v.is_string() != 0 => {
                    let s = v8_to_string(v);
                    if s.is_empty() {
                        if let Some(exc) = exception { *exc = CefString::from("command cannot be empty"); }
                        return 0;
                    }
                    s
                }
                _ => {
                    if let Some(exc) = exception { *exc = CefString::from("command must be a non-empty string"); }
                    return 0;
                }
            };

            let context = match v8_context_get_current_context() {
                Some(ctx) => ctx,
                None => {
                    if let Some(exc) = exception {
                        *exc = CefString::from("invoke: no active renderer context");
                    }
                    return 0;
                }
            };

            let Some(frame) = context.frame() else {
                if let Some(exc) = exception {
                    *exc = CefString::from("invoke: no frame for current context");
                }
                return 0;
            };

            // Detect payload type from second argument
            let is_binary = args.get(1)
                .and_then(|v| v.as_ref())
                .is_some_and(|v| v.is_array_buffer() != 0);

            let Some(cmd_header) = encode_cmd_header(&cmd) else {
                if let Some(exc) = exception {
                    *exc = CefString::from(
                        "invoke: command name exceeds the 65535-byte protocol limit",
                    );
                }
                return 0;
            };

            let Some(promise) = v8_value_create_promise() else {
                if let Some(exc) = exception { *exc = CefString::from("invoke: cannot create a promise"); }
                return 0;
            };
            let promise_for_retval = promise.clone();
            let (id, flags) = {
                let mut state = state();
                (state.register_rpc(&context, promise.clone()), state.flags_for(&context))
            };
            let Some(id) = id else {
                if let Some(exc) = exception { *exc = CefString::from("invoke: this context is not connected"); }
                return 0;
            };

            // Expose the id on the promise for JS-side cancellation
            let id_key = CefString::from("__kurogane_id");
            let mut id_value = v8_value_create_int(id).unwrap();
            promise_for_retval.set_value_bykey(
                Some(&id_key),
                Some(&mut id_value),
                V8Propertyattribute::default(),
            );

            debug!("[IPC Renderer] JS invoke: '{}' (id={}, binary={})", cmd, id, is_binary);

            if is_binary {
                let buffer = args.get(1).unwrap().as_ref().unwrap();
                let ptr = buffer.array_buffer_data();
                let len = buffer.array_buffer_byte_length();

                if ptr.is_null() {
                    if let Some(exc) = exception {
                        *exc = CefString::from("ArrayBuffer has null data");
                    }
                    state().forget(id);
                    return 0;
                }

                let mut build_failed = false;
                with_array_buffer(ptr as *const u8, len, |data| {
                    let envelope = Envelope {
                        version: ENVELOPE_VERSION,
                        subsystem: SUB_RPC,
                        opcode: RPC_INVOKE,
                        flags,
                        correlation_id: id as u32,
                        payload_kind: PAYLOAD_BINARY,
                    };

                    if let Some(mut msg) = build_message_parts("kurogane_rpc", &envelope, &[&cmd_header, data]) {
                        frame.send_process_message(ProcessId::BROWSER, Some(&mut msg));
                    } else {
                        build_failed = true;
                        state().forget(id);
                        let reject_msg = CefString::from("-1: Failed to build IPC message");
                        promise.reject_promise(Some(&reject_msg));
                    }
                });

                if build_failed {
                    if let Some(ret) = retval { *ret = Some(promise_for_retval); }
                    return 1;
                }
            } else {
                let payload = match args.get(1) {
                    Some(Some(v)) if v.is_string() != 0 => v8_to_string(v),
                    _ => String::new(),
                };

                let envelope = Envelope {
                    version: ENVELOPE_VERSION,
                    subsystem: SUB_RPC,
                    opcode: RPC_INVOKE,
                    flags,
                    correlation_id: id as u32,
                    payload_kind: PAYLOAD_STRING,
                };

                let payload_bytes = payload.as_bytes();

                if let Some(mut msg) = build_message_parts("kurogane_rpc", &envelope, &[&cmd_header, payload_bytes]) {
                    frame.send_process_message(ProcessId::BROWSER, Some(&mut msg));
                } else {
                    state().forget(id);
                    let reject_msg = CefString::from("-1: Failed to build IPC message");
                    promise.reject_promise(Some(&reject_msg));
                    if let Some(ret) = retval { *ret = Some(promise_for_retval); }
                    return 1;
                }
            }

            if let Some(ret) = retval {
                *ret = Some(promise_for_retval);
            }

            1
        }
    }
}

//
// Cancel handler
//

wrap_v8_handler! {
    pub struct IpcCancelHandler;

    impl V8Handler {
        fn execute(
            &self,
            _name: Option<&CefString>,
            _object: Option<&mut V8Value>,
            arguments: Option<&[Option<V8Value>]>,
            retval: Option<&mut Option<V8Value>>,
            exception: Option<&mut CefString>,
        ) -> i32 {
            let args = match arguments {
                Some(a) if !a.is_empty() => a,
                _ => {
                    if let Some(exc) = exception {
                        *exc = CefString::from("cancel requires an id argument");
                    }
                    return 0;
                }
            };

            let id = match args.first() {
                Some(Some(v)) if v.is_int() != 0 || v.is_uint() != 0 => v.int_value(),
                _ => {
                    if let Some(exc) = exception {
                        *exc = CefString::from("cancel: id must be an integer");
                    }
                    return 0;
                }
            };

            let Some(context) = v8_context_get_current_context() else {
                if let Some(ret) = retval { *ret = v8_value_create_bool(0); }
                return 1;
            };
            // Only the context that made a request can cancel it
            let (cancelled, flags) = {
                let mut state = state();
                (state.cancel(id, &context), state.flags_for(&context))
            };
            let Some((owner, promise, sub)) = cancelled else {
                if let Some(ret) = retval { *ret = v8_value_create_bool(0); }
                return 1;
            };

            let opcode = match sub {
                SUB_STREAM => STREAM_CANCEL,
                _ => RPC_CANCEL,
            };
            let envelope = Envelope {
                version: ENVELOPE_VERSION,
                subsystem: sub,
                opcode,
                flags,
                correlation_id: id as u32,
                payload_kind: PAYLOAD_EMPTY,
            };
            if let Some(frame) = context.frame()
                && let Some(mut msg) = build_message("kurogane_rpc", &envelope, &[])
            {
                frame.send_process_message(ProcessId::BROWSER, Some(&mut msg));
            }

            if owner.enter() == 0 {
                debug!("[IPC Renderer] cancel: failed to enter V8 context for id={}", id);
                return 1;
            }
            let reject_msg = CefString::from("0: Canceled");
            promise.reject_promise(Some(&reject_msg));
            owner.exit();
            if let Some(ret) = retval {
                *ret = v8_value_create_bool(1);
            }

            1
        }
    }
}

//
// Event on handler
//

wrap_v8_handler! {
    pub struct IpcOnHandler;

    impl V8Handler {
        fn execute(
            &self,
            _name: Option<&CefString>,
            _object: Option<&mut V8Value>,
            arguments: Option<&[Option<V8Value>]>,
            retval: Option<&mut Option<V8Value>>,
            exception: Option<&mut CefString>,
        ) -> i32 {
            let args = match arguments {
                Some(a) if a.len() >= 2 => a,
                _ => {
                    if let Some(exc) = exception {
                        *exc = CefString::from("on(eventName, callback[, onError]) requires two arguments");
                    }
                    return 0;
                }
            };

            let event_name = match args.first() {
                Some(Some(v)) if v.is_string() != 0 => v8_to_string(v),
                _ => {
                    if let Some(exc) = exception { *exc = CefString::from("event name must be a string"); }
                    return 0;
                }
            };

            if event_name.is_empty() {
                if let Some(exc) = exception { *exc = CefString::from("event name cannot be empty"); }
                return 0;
            }

            let callback = match args.get(1) {
                Some(Some(v)) if v.is_function() != 0 => v,
                _ => {
                    if let Some(exc) = exception { *exc = CefString::from("second argument must be a function"); }
                    return 0;
                }
            };

            // Optional callback invoked when the subscription is refused
            let on_error = match args.get(2) {
                Some(Some(v)) if v.is_function() != 0 => Some(v.clone()),
                Some(Some(v)) if v.is_undefined() == 0 && v.is_null() == 0 => {
                    if let Some(exc) = exception { *exc = CefString::from("third argument must be a function"); }
                    return 0;
                }
                _ => None,
            };

            let context = match v8_context_get_current_context() {
                Some(ctx) => ctx,
                None => {
                    if let Some(exc) = exception { *exc = CefString::from("on: no active renderer context"); }
                    return 0;
                }
            };

            let Some(frame) = context.frame() else {
                if let Some(exc) = exception { *exc = CefString::from("on: no frame for current context"); }
                return 0;
            };

            let Some(payload) = encode_cmd_payload(&event_name, &[]) else {
                if let Some(exc) = exception {
                    *exc = CefString::from(
                        "on: event name exceeds the 65535-byte protocol limit",
                    );
                }
                return 0;
            };

            let (id, flags) = {
                let mut state = state();
                let id = state.subscribe(&context, event_name.clone(), callback.clone(), on_error);
                (id, state.flags_for(&context))
            };
            let Some(id) = id else {
                if let Some(exc) = exception { *exc = CefString::from("on: this context is not connected"); }
                return 0;
            };

            let subscribe = {
                let envelope = Envelope {
                    version: ENVELOPE_VERSION,
                    subsystem: SUB_EVENT,
                    opcode: EVENT_SUBSCRIBE,
                    flags,
                    correlation_id: id as u32,
                    payload_kind: PAYLOAD_EMPTY,
                };
                build_message("kurogane_event", &envelope, &payload)
            };

            if let Some(mut msg) = subscribe {
                frame.send_process_message(ProcessId::BROWSER, Some(&mut msg));
            } else {
                debug!("[IPC Renderer] failed to build subscribe message");
            }

            debug!("[IPC Renderer] event on '{}' id={}", event_name, id);

            if let Some(ret) = retval {
                *ret = v8_value_create_uint(id as u32);
            }

            1
        }
    }
}

//
// Event off handler
//

wrap_v8_handler! {
    pub struct IpcOffHandler;

    impl V8Handler {
        fn execute(
            &self,
            _name: Option<&CefString>,
            _object: Option<&mut V8Value>,
            arguments: Option<&[Option<V8Value>]>,
            retval: Option<&mut Option<V8Value>>,
            exception: Option<&mut CefString>,
        ) -> i32 {
            let args = match arguments {
                Some(a) if !a.is_empty() => a,
                _ => {
                    if let Some(exc) = exception { *exc = CefString::from("off requires an id argument"); }
                    return 0;
                }
            };

            let id = match args.first() {
                Some(Some(v)) if v.is_int() != 0 || v.is_uint() != 0 => v.int_value(),
                _ => {
                    if let Some(exc) = exception { *exc = CefString::from("off: id must be an integer"); }
                    return 0;
                }
            };

            let context = match v8_context_get_current_context() {
                Some(ctx) => ctx,
                None => {
                    if let Some(exc) = exception { *exc = CefString::from("off: no active renderer context"); }
                    return 0;
                }
            };

            // Only the context that subscribed can unsubscribe
            let (event_name, flags) = {
                let mut state = state();
                (state.unsubscribe(id, &context), state.flags_for(&context))
            };
            let was_valid = event_name.is_some();

            if let Some(event_name) = event_name
                && let Some(frame) = context.frame()
                && let Some(payload) = encode_cmd_payload(&event_name, &[])
            {
                let envelope = Envelope {
                    version: ENVELOPE_VERSION,
                    subsystem: SUB_EVENT,
                    opcode: EVENT_UNSUBSCRIBE,
                    flags,
                    correlation_id: id as u32,
                    payload_kind: PAYLOAD_EMPTY,
                };
                if let Some(mut msg) = build_message("kurogane_event", &envelope, &payload) {
                    frame.send_process_message(ProcessId::BROWSER, Some(&mut msg));
                }
            }

            if let Some(ret) = retval {
                *ret = v8_value_create_bool(if was_valid { 1 } else { 0 });
            }
            1
        }
    }
}

//
// Stream open handler
//

wrap_v8_handler! {
    pub struct IpcOpenStreamHandler;

    impl V8Handler {
        fn execute(
            &self,
            _name: Option<&CefString>,
            _object: Option<&mut V8Value>,
            arguments: Option<&[Option<V8Value>]>,
            retval: Option<&mut Option<V8Value>>,
            exception: Option<&mut CefString>,
        ) -> i32 {
            let args = match arguments {
                Some(a) if !a.is_empty() => a,
                _ => {
                    if let Some(exc) = exception { *exc = CefString::from("openStream requires a handler name argument"); }
                    return 0;
                }
            };

            let handler_name = match args.first() {
                Some(Some(v)) if v.is_string() != 0 => v8_to_string(v),
                _ => {
                    if let Some(exc) = exception { *exc = CefString::from("handler name must be a string"); }
                    return 0;
                }
            };

            if handler_name.is_empty() {
                if let Some(exc) = exception { *exc = CefString::from("handler name cannot be empty"); }
                return 0;
            }

            let metadata = match args.get(1) {
                Some(Some(v)) if v.is_string() != 0 => v8_to_string(v),
                _ => String::new(),
            };

            // The stream's callbacks are bound here, atomically with the open,
            // so no other frame can ever attach to this stream
            let function = |i: usize| match args.get(i) {
                Some(Some(v)) if v.is_function() != 0 => Some(v.clone()),
                _ => None,
            };
            let (Some(data), Some(end), Some(error)) = (function(2), function(3), function(4)) else {
                if let Some(exc) = exception {
                    *exc = CefString::from("openStream(name, metadata, onData, onEnd, onError) needs three callbacks");
                }
                return 0;
            };

            let context = match v8_context_get_current_context() {
                Some(ctx) => ctx,
                None => {
                    if let Some(exc) = exception { *exc = CefString::from("openStream: no active renderer context"); }
                    return 0;
                }
            };

            let Some(frame) = context.frame() else {
                if let Some(exc) = exception { *exc = CefString::from("openStream: no frame for current context"); }
                return 0;
            };

            let Some(promise) = v8_value_create_promise() else {
                if let Some(exc) = exception { *exc = CefString::from("openStream: cannot create a promise"); }
                return 0;
            };
            let (stream_id, flags) = {
                let mut state = state();
                let sink = StreamSink { data, end, error };
                (state.register_stream_open(&context, promise.clone(), sink), state.flags_for(&context))
            };
            let Some(stream_id) = stream_id else {
                if let Some(exc) = exception { *exc = CefString::from("openStream: this context is not connected"); }
                return 0;
            };

            debug!("[IPC Renderer] openStream '{}' stream_id={}", handler_name, stream_id);

            let envelope = Envelope {
                version: ENVELOPE_VERSION,
                subsystem: SUB_STREAM,
                opcode: STREAM_OPEN,
                flags,
                correlation_id: stream_id as u32,
                payload_kind: PAYLOAD_STRING,
            };

            let sent = if let Some(payload) = encode_cmd_payload(&handler_name, metadata.as_bytes())
                && let Some(mut msg) = build_message("kurogane_stream", &envelope, &payload)
            {
                frame.send_process_message(ProcessId::BROWSER, Some(&mut msg));
                true
            } else {
                false
            };
            if !sent {
                state().forget(stream_id);
                let reject_msg = CefString::from("-1: Failed to build IPC message");
                promise.reject_promise(Some(&reject_msg));
            }

            // The promise represents completion of the open request, not message delivery
            // It is resolved or rejected when the browser responds

            if let Some(ret) = retval {
                *ret = Some(promise);
            }

            1
        }
    }
}

//
// Stream write handler
//

wrap_v8_handler! {
    pub struct IpcWriteStreamHandler;

    impl V8Handler {
        fn execute(
            &self,
            _name: Option<&CefString>,
            _object: Option<&mut V8Value>,
            arguments: Option<&[Option<V8Value>]>,
            retval: Option<&mut Option<V8Value>>,
            exception: Option<&mut CefString>,
        ) -> i32 {
            let args = match arguments {
                Some(a) if a.len() >= 2 => a,
                _ => {
                    if let Some(exc) = exception { *exc = CefString::from("writeStream(streamId, ArrayBuffer)"); }
                    return 0;
                }
            };

            let stream_id = match args.first() {
                Some(Some(v)) if v.is_int() != 0 || v.is_uint() != 0 => v.int_value(),
                _ => {
                    if let Some(exc) = exception { *exc = CefString::from("writeStream: streamId must be an integer"); }
                    return 0;
                }
            };

            let buffer = match args.get(1) {
                Some(Some(v)) if v.is_array_buffer() != 0 => v,
                _ => {
                    if let Some(exc) = exception { *exc = CefString::from("writeStream: second argument must be an ArrayBuffer"); }
                    return 0;
                }
            };

            let ptr = buffer.array_buffer_data();
            let len = buffer.array_buffer_byte_length();

            if ptr.is_null() {
                if let Some(exc) = exception { *exc = CefString::from("writeStream: ArrayBuffer has null data"); }
                return 0;
            }

            let context = match v8_context_get_current_context() {
                Some(ctx) => ctx,
                None => {
                    if let Some(exc) = exception { *exc = CefString::from("writeStream: no active renderer context"); }
                    return 0;
                }
            };

            let Some(frame) = context.frame() else {
                if let Some(exc) = exception { *exc = CefString::from("writeStream: no frame for current context"); }
                return 0;
            };

            let (owned, flags) = {
                let state = state();
                (state.owns_stream(stream_id, &context), state.flags_for(&context))
            };
            if !owned {
                if let Some(ret) = retval { *ret = v8_value_create_bool(0); }
                return 1;
            }

            with_array_buffer(ptr as *const u8, len, |data| {
                let envelope = Envelope {
                    version: ENVELOPE_VERSION,
                    subsystem: SUB_STREAM,
                    opcode: STREAM_DATA,
                    flags,
                    correlation_id: stream_id as u32,
                    payload_kind: PAYLOAD_BINARY,
                };

                if let Some(mut msg) = build_message("kurogane_stream", &envelope, data) {
                    frame.send_process_message(ProcessId::BROWSER, Some(&mut msg));
                }
            });

            if let Some(ret) = retval {
                *ret = v8_value_create_uint(1);
            }

            1
        }
    }
}

//
// Stream end handler
//

wrap_v8_handler! {
    pub struct IpcEndStreamHandler;

    impl V8Handler {
        fn execute(
            &self,
            _name: Option<&CefString>,
            _object: Option<&mut V8Value>,
            arguments: Option<&[Option<V8Value>]>,
            retval: Option<&mut Option<V8Value>>,
            exception: Option<&mut CefString>,
        ) -> i32 {
            let args = match arguments {
                Some(a) if !a.is_empty() => a,
                _ => {
                    if let Some(exc) = exception { *exc = CefString::from("endStream requires a streamId argument"); }
                    return 0;
                }
            };

            let stream_id = match args.first() {
                Some(Some(v)) if v.is_int() != 0 || v.is_uint() != 0 => v.int_value(),
                _ => {
                    if let Some(exc) = exception { *exc = CefString::from("endStream: streamId must be an integer"); }
                    return 0;
                }
            };

            let result = match args.get(1) {
                Some(Some(v)) if v.is_string() != 0 => v8_to_string(v),
                _ => String::new(),
            };

            let context = match v8_context_get_current_context() {
                Some(ctx) => ctx,
                None => {
                    if let Some(exc) = exception { *exc = CefString::from("endStream: no active renderer context"); }
                    return 0;
                }
            };

            let Some(frame) = context.frame() else {
                if let Some(exc) = exception { *exc = CefString::from("endStream: no frame for current context"); }
                return 0;
            };

            let (owned, flags) = {
                let state = state();
                (state.owns_stream(stream_id, &context), state.flags_for(&context))
            };
            if !owned {
                if let Some(ret) = retval { *ret = v8_value_create_bool(0); }
                return 1;
            }

            let payload = result.as_bytes();

            let envelope = Envelope {
                version: ENVELOPE_VERSION,
                subsystem: SUB_STREAM,
                opcode: STREAM_END,
                flags,
                correlation_id: stream_id as u32,
                payload_kind: PAYLOAD_STRING,
            };

            if let Some(mut msg) = build_message("kurogane_stream", &envelope, payload) {
                frame.send_process_message(ProcessId::BROWSER, Some(&mut msg));
            }

            if let Some(ret) = retval {
                *ret = v8_value_create_uint(1);
            }

            1
        }
    }
}
