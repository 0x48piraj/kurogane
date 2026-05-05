//! V8 bridge for IPC
//!
//! Connects JavaScript to the browser process via CEF messages.
//! Defines the boundary between JavaScript and the native IPC system.

use cef::*;
use crate::debug;
use crate::ipc::protocol::{set_kind, IpcMsgKind};
use crate::ipc::transport::shm::{SharedBuffer, SHM_THRESHOLD, SHM_HEADER_SIZE};
use crate::ipc::renderer_state::{register_promise, clear_context_promises, outgoing_shm};
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

//
// Renderer process handler
//

wrap_render_process_handler! {
    pub struct IpcRenderProcessHandler;

    impl RenderProcessHandler {
        fn on_context_created(
            &self,
            _browser: Option<&mut Browser>,
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
        }

        fn on_context_released(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            context: Option<&mut V8Context>,
        ) {
            // cleanup
            if let Some(ctx) = context {
                clear_context_promises(ctx);
            }
        }

        fn on_process_message_received(
            &self,
            _browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            source_process: ProcessId,
            message: Option<&mut ProcessMessage>,
        ) -> i32 {
            if source_process != ProcessId::BROWSER { return 0; }
            let msg = message.unwrap();

            let name: CefString = (&msg.name()).into();
            if name.to_string() != "ipc" { return 0; }

            let Some(args) = msg.argument_list() else {
                debug!("[IPC Renderer] missing argument list");
                return 0;
            };

            let Some(frame) = frame else {
                debug!("[IPC Renderer] missing frame");
                return 0;
            };

            // Always call router for valid IPC message
            let handled = router::route_renderer(frame, &args);

            if !handled {
                debug!("[IPC Renderer] unexpected ipc message type from browser");
            }

            1
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
            let cmd = match args.get(0) {
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

            let id = register_promise(context.clone(), promise.clone());

            debug!("[IPC Renderer] JS invoke: '{}' (id={})", cmd, id);

            let mut msg = process_message_create(Some(&CefString::from("ipc"))).unwrap();
            let mut msg_args = msg.argument_list().unwrap();

            set_kind(&mut msg_args, IpcMsgKind::Invoke);
            msg_args.set_int(1, id as i32);
            msg_args.set_string(2, Some(&CefString::from(cmd.as_str())));
            msg_args.set_string(3, Some(&CefString::from(payload.as_str())));

            frame.send_process_message(ProcessId::BROWSER, Some(&mut msg));

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

            let cmd = match args.get(0) {
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

            // Build message first (no id yet)
            let mut msg = process_message_create(Some(&CefString::from("ipc"))).unwrap();
            let mut msg_args = msg.argument_list().unwrap();

            set_kind(&mut msg_args, IpcMsgKind::BinaryInvoke);
            msg_args.set_string(2, Some(&CefString::from(cmd.as_str())));

            // Build payload before committing to promise/id
            let payload_result: Result<Option<SharedBuffer>, String> =
                with_array_buffer(ptr as *const u8, len, |data| {
                    if len < SHM_THRESHOLD {
                        // inline: faster for small-medium sizes
                        let mut binary = binary_value_create(Some(data)).unwrap();
                        msg_args.set_binary(3, Some(&mut binary));
                        Ok(None)
                    } else {
                        // shm: only for large payloads
                        let mut shm = SharedBuffer::create(len)?;
                        shm.write(data)?;

                        let name = shm.name();
                        msg_args.set_string(3, Some(&CefString::from(name.as_str())));
                        msg_args.set_int(4, (len + SHM_HEADER_SIZE) as i32);

                        Ok(Some(shm))
                    }
                });

            let shm = match payload_result {
                Ok(shm) => shm,
                Err(e) => {
                    let msg = CefString::from(e.as_str());
                    // Payload construction failed before promise registration.
                    // Reject directly instead of going through the registry (no id exists yet).
                    promise.reject_promise(Some(&msg));
                    // Reject and return the promise so the JS caller observes the failure.
                    // The promise is not yet registered, so it must be returned directly.
                    if let Some(ret) = retval {
                        *ret = Some(promise);
                    }
                    return 1;
                }
            };

            // Commit
            let id = register_promise(context.clone(), promise.clone());
            msg_args.set_int(1, id as i32);

            // Store SHM only after id exists
            if let Some(shm) = shm {
                outgoing_shm().lock().unwrap().insert(id, shm);
            }

            frame.send_process_message(ProcessId::BROWSER, Some(&mut msg));

            if let Some(ret) = retval {
                *ret = Some(promise);
            }

            1
        }
    }
}
