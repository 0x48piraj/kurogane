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
fn with_array_buffer<R>(
    ptr: *const u8,
    len: usize,
    f: impl FnOnce(&[u8]) -> R,
) -> R {
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
    let slice = unsafe {
        std::slice::from_raw_parts(ptr, len)
    };

    f(slice)
}

/// Encode [cmd_len:u16 LE][cmd_bytes] into a Vec.
fn encode_cmd_header(cmd: &str) -> Vec<u8> {
    let cmd_bytes = cmd.as_bytes();
    let mut v = Vec::with_capacity(2 + cmd_bytes.len());
    v.extend_from_slice(&(cmd_bytes.len() as u16).to_le_bytes());
    v.extend_from_slice(cmd_bytes);
    v
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
            let context = context.unwrap();
            let frame = frame.unwrap();

            let global = context.global().unwrap();
            let mut core = v8_value_create_object(None, None).unwrap();

            // JSON invoke
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

            // Binary invoke
            let mut bin_handler = IpcInvokeBinaryHandler::new();
            let mut invoke_binary = v8_value_create_function(
                Some(&CefString::from("invokeBinary")),
                Some(&mut bin_handler),
            ).unwrap();

            core.set_value_bykey(
                Some(&CefString::from("invokeBinary")),
                Some(&mut invoke_binary),
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
                clear_context_promises(ctx);
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

            router::route_renderer(frame, received.as_bytes());
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
// JSON invoke handler
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

            // optional payload (string)
            let payload = match args.get(1) {
                Some(Some(v)) if v.is_string() != 0 => {
                    v8_to_string(v)
                }
                _ => String::new(),
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

            let promise = v8_value_create_promise().unwrap();

            let id = register_promise(context.clone(), promise.clone(), SUB_RPC);

            debug!("[IPC Renderer] JS invoke: '{}' (id={})", cmd, id);

            let envelope = Envelope {
                version: ENVELOPE_VERSION,
                subsystem: SUB_RPC,
                opcode: RPC_INVOKE,
                flags: 0,
                correlation_id: id as u32,
                payload_kind: PAYLOAD_STRING,
            };

            // Build parts separately to avoid intermediate Vec
            let cmd_header = encode_cmd_header(&cmd);
            let payload_bytes = payload.as_bytes();

            if let Some(mut msg) = build_message_parts("kurogane_rpc", &envelope, &[&cmd_header, payload_bytes]) {
                frame.send_process_message(ProcessId::BROWSER, Some(&mut msg));
            } else {
                if context.enter() == 0 {
                    registry().lock().unwrap().take(id);
                    return 0;
                }
                let reject_msg = CefString::from("ERR_-1: Failed to build IPC message");
                promise.reject_promise(Some(&reject_msg));
                context.exit();
                registry().lock().unwrap().take(id);
                if let Some(ret) = retval {
                    *ret = Some(promise);
                }
                return 1;
            }

            if let Some(ret) = retval {
                *ret = Some(promise);
            }

            1
        }
    }
}

//
// Binary invoke handler
//

wrap_v8_handler! {
    pub struct IpcInvokeBinaryHandler;

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
                    if let Some(exc) = exception { *exc = CefString::from("invokeBinary(command, ArrayBuffer)"); }
                    return 0;
                }
            };

            let cmd = match args.first() {
                Some(Some(v)) if v.is_string() != 0 => {
                    v8_to_string(v)
                }
                _ => {
                    if let Some(exc) = exception {
                        *exc = CefString::from("command must be a string");
                    }
                    return 0;
                }
            };

            // Accept ArrayBuffer only.
            // Callers must pass data.buffer (not a Uint8Array view) enforced in the JS wrapper.
            let buffer = match args.get(1) {
                Some(Some(v)) if v.is_array_buffer() != 0 => v,
                _ => {
                    if let Some(exc) = exception {
                        *exc = CefString::from("second argument must be an ArrayBuffer (use invokeBinary())");
                    }
                    return 0;
                }
            };

            let ptr = buffer.array_buffer_data();
            let len = buffer.array_buffer_byte_length();

            if ptr.is_null() {
                if let Some(exc) = exception {
                    *exc = CefString::from("ArrayBuffer has null data");
                }
                return 0;
            }

            let context = match v8_context_get_current_context() {
                Some(ctx) => ctx,
                None => {
                    if let Some(exc) = exception {
                        *exc = CefString::from("invokeBinary: no active renderer context");
                    }
                    return 0;
                }
            };

            let Some(frame) = context.frame() else {
                if let Some(exc) = exception {
                    *exc = CefString::from("invokeBinary: no frame for current context");
                }
                return 0;
            };

            let promise = v8_value_create_promise().unwrap();
            let promise_for_retval = promise.clone();
            let id = register_promise(context.clone(), promise.clone(), SUB_BINARY);

            let mut build_failed = false;
            with_array_buffer(ptr as *const u8, len, |data| {
                let cmd_header = encode_cmd_header(&cmd);

                let envelope = Envelope {
                    version: ENVELOPE_VERSION,
                    subsystem: SUB_BINARY,
                    opcode: BINARY_INVOKE,
                    flags: 0,
                    correlation_id: id as u32,
                    payload_kind: PAYLOAD_BINARY,
                };

                if let Some(mut msg) = build_message_parts("kurogane_binary", &envelope, &[&cmd_header, data]) {
                    frame.send_process_message(ProcessId::BROWSER, Some(&mut msg));
                } else {
                    build_failed = true;
                    if let Some(ctx) = v8_context_get_current_context() {
                        if ctx.enter() == 0 {
                            registry().lock().unwrap().take(id);
                            return;
                        }
                        let reject_msg = CefString::from("ERR_-1: Failed to build IPC message");
                        promise.reject_promise(Some(&reject_msg));
                        ctx.exit();
                    }
                    registry().lock().unwrap().take(id);
                }
            });

            if build_failed {
                if let Some(ret) = retval {
                    *ret = Some(promise_for_retval);
                }
                return 1;
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

            // Send CancelRequest to browser so it can abort the pending async handler
            if let Some(context) = v8_context_get_current_context() {
                if let Some(frame) = context.frame() {
                    let mut msg = process_message_create(Some(&CefString::from("ipc"))).unwrap();
                    let mut msg_args = msg.argument_list().unwrap();
                    set_kind(&mut msg_args, IpcMsgKind::CancelRequest);
                    msg_args.set_int(1, id);
                    frame.send_process_message(ProcessId::BROWSER, Some(&mut msg));
                }
            }

            if let Some((ctx, promise)) = cancel_promise(id) {
                if ctx.enter() == 0 {
                    eprintln!("[IPC] cancel: failed to enter V8 context for promise id={}", id);
                    return 1;
                }
                let reject_msg = CefString::from("ERR_0: Canceled");
                promise.reject_promise(Some(&reject_msg));
                ctx.exit();
                if let Some(ret) = retval {
                    *ret = v8_value_create_bool(1);
                }
            } else {
                if let Some(ret) = retval {
                    *ret = v8_value_create_bool(0);
                }
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
                        *exc = CefString::from("on(eventName, callback) requires two arguments");
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

            let context = match v8_context_get_current_context() {
                Some(ctx) => ctx,
                None => {
                    if let Some(exc) = exception { *exc = CefString::from("on: no active renderer context"); }
                    return 0;
                }
            };

            let id = event_registry().lock().unwrap().register(
                &event_name,
                context.clone(),
                callback.clone(),
            );

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
                Some(Some(v)) if v.is_int() != 0 || v.is_uint() != 0 => v.int_value() as i64,
                _ => {
                    if let Some(exc) = exception { *exc = CefString::from("off: id must be an integer"); }
                    return 0;
                }
            };

            let removed = event_registry().lock().unwrap().unregister(id);
            if let Some(ret) = retval {
                *ret = v8_value_create_bool(if removed { 1 } else { 0 });
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

            let promise = v8_value_create_promise().unwrap();
            let stream_id = register_promise(context.clone(), promise.clone(), SUB_STREAM);

            debug!("[IPC Renderer] openStream '{}' stream_id={}", handler_name, stream_id);

            let envelope = Envelope {
                version: ENVELOPE_VERSION,
                subsystem: SUB_STREAM,
                opcode: STREAM_OPEN,
                flags: 0,
                correlation_id: stream_id as u32,
                payload_kind: PAYLOAD_STRING,
            };

            let payload = encode_cmd_payload(&handler_name, metadata.as_bytes());
            if let Some(mut msg) = build_message("kurogane_stream", &envelope, &payload) {
                frame.send_process_message(ProcessId::BROWSER, Some(&mut msg));
            } else {
                if context.enter() == 0 {
                    registry().lock().unwrap().take(stream_id);
                    return 0;
                }
                let reject_msg = CefString::from("ERR_-1: Failed to build IPC message");
                promise.reject_promise(Some(&reject_msg));
                context.exit();
                registry().lock().unwrap().take(stream_id);
                if let Some(ret) = retval {
                    *ret = Some(promise);
                }
                return 1;
            }

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

            with_array_buffer(ptr as *const u8, len, |data| {
                let envelope = Envelope {
                    version: ENVELOPE_VERSION,
                    subsystem: SUB_STREAM,
                    opcode: STREAM_DATA,
                    flags: 0,
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

            let payload = result.as_bytes();

            let envelope = Envelope {
                version: ENVELOPE_VERSION,
                subsystem: SUB_STREAM,
                opcode: STREAM_END,
                flags: 0,
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
