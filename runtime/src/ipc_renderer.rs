//! Renderer process IPC implementation.
//! Transport uses ProcessMessage "ipc" and ListValue typed args.

use cef::*;
use std::sync::{Mutex, OnceLock};
use std::collections::HashMap;

use crate::ipc_shm::{SharedBuffer, SHM_THRESHOLD};
use crate::debug;

//
// Promise registry: Tracks pending promises awaiting responses from the browser process
//

struct PromiseRegistry {
    next_id: u32,
    pending: HashMap<u32, (V8Context, V8Value)>,
}

impl PromiseRegistry {
    fn new() -> Self { Self { next_id: 1, pending: HashMap::new() } }

    fn register(&mut self, context: V8Context, promise: V8Value) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);

        self.pending.insert(id, (context, promise));
        id
    }

    fn resolve_string(&mut self, id: u32, success: bool, result: &str) {

        if let Some((context, promise)) = self.pending.remove(&id) {
            if context.enter() == 0 {
                eprintln!("[IPC] Failed to enter V8 context for promise id={}", id);
                return;
            }

            let s = CefString::from(result);

            if success {
                // resolve promise with string value
                let mut v = v8_value_create_string(Some(&s)).unwrap();
                promise.resolve_promise(Some(&mut v));
            } else {
                // reject with exception string
                promise.reject_promise(Some(&s));
            }

            context.exit();
        } else {
            eprintln!("[IPC WARNING] response for unknown promise id={} (likely page reload)", id);
        }
    }

    fn resolve_binary(&mut self, id: u32, data: &[u8]) {
        if let Some((context, promise)) = self.pending.remove(&id) {
            if context.enter() == 0 { return; }
            let mut buf = v8_value_create_array_buffer_with_copy(
                data.as_ptr() as *mut u8,
                data.len(),
            ).unwrap();

            promise.resolve_promise(Some(&mut buf));

            context.exit();
        }
    }
}

static PROMISE_REGISTRY: OnceLock<Mutex<PromiseRegistry>> = OnceLock::new();

fn ensure_registry() {
    PROMISE_REGISTRY.get_or_init(|| Mutex::new(PromiseRegistry::new()));
}

fn registry() -> &'static Mutex<PromiseRegistry> {
    PROMISE_REGISTRY.get().unwrap()
}

fn register_promise(ctx: V8Context, promise: V8Value) -> u32 {
    registry().lock().unwrap().register(ctx, promise)
}

fn resolve_string(id: u32, success: bool, payload: &str) {
    registry().lock().unwrap().resolve_string(id, success, payload)
}

fn resolve_binary(id: u32, payload: &[u8]) {
    registry().lock().unwrap().resolve_binary(id, payload)
}

fn clear_context_promises(ctx: &V8Context) {
    let mut r = registry().lock().unwrap();
    r.pending.retain(|_, (stored_ctx, _)| {
        let mut other = ctx.clone();
        stored_ctx.is_same(Some(&mut other)) == 0
    });

    println!("[IPC] cleared promises for destroyed JS context");
}

//
// SHM store for renderer→browser outgoing requests.
// Keeps the SHM alive until the browser's response arrives,
// proving the browser has already read the data.
//

static OUTGOING_SHM: OnceLock<Mutex<HashMap<u32, SharedBuffer>>> = OnceLock::new();

fn outgoing_shm() -> &'static Mutex<HashMap<u32, SharedBuffer>> {
    OUTGOING_SHM.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Notify the browser that it can release its SHM response buffer.
fn send_shm_free(id: u32, frame: &mut Frame) {
    let mut msg = process_message_create(Some(&CefString::from("ipc"))).unwrap();
    let args = msg.argument_list().unwrap();
    args.set_int(0, 5); // SHM_FREE
    args.set_int(1, id as i32);
    frame.send_process_message(ProcessId::BROWSER, Some(&mut msg));
    debug!("[Renderer] SHM_FREE sent for id={}", id);
}

//
// Helpers
//

fn list_int(args: &ListValue, idx: usize) -> i32 { args.int(idx) }

fn list_string(args: &ListValue, idx: usize) -> String {
    let s: CefString = (&args.string(idx)).into();
    s.to_string()
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
            _frame: Option<&mut Frame>,
            context: Option<&mut V8Context>,
        ) {
            ensure_registry();

            let context = context.unwrap();
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

            debug!("[Renderer] Injected window.core.invoke + invokeBinary");
        }

        fn on_context_released(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            context: Option<&mut V8Context>,
        ) {
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

            let args = msg.argument_list().unwrap();

            let msg_type = list_int(&args, 0);
            let id = list_int(&args, 1) as u32;

            match msg_type {
                1 => {
                    // Release outgoing SHM; browser has read it and responded
                    outgoing_shm().lock().unwrap().remove(&id);
                    let payload = list_string(&args, 2);
                    resolve_string(id, true, &payload);
                }

                2 => {
                    outgoing_shm().lock().unwrap().remove(&id);
                    let payload = list_string(&args, 2);
                    resolve_string(id, false, &payload);
                }

                4 => {
                    // Release outgoing SHM regardless of transport used in response
                    outgoing_shm().lock().unwrap().remove(&id);

                    if let Some(binary) = args.binary(2) {

                        let size = binary.size();
                        let mut buf = vec![0u8; size];

                        let written = binary.data(Some(&mut buf), 0);
                        buf.truncate(written);

                        debug!("[Renderer] inline binary response: {} bytes", written);

                        resolve_binary(id, &buf);
                    } else {
                        // Browser used SHM for this response
                        let name = list_string(&args, 2);
                        let size = list_int(&args, 3) as usize;

                        // Copy data before dropping the SHM mapping
                        let data_vec = match SharedBuffer::open(&name, size) {
                            Ok(shm)  => shm.as_slice().to_vec(),
                            Err(e)   => {
                                eprintln!("[IPC] SHM open failed for id={}: {}", id, e);
                                resolve_string(id, false, &format!("shm transport error: {}", e));
                                if let Some(f) = frame { send_shm_free(id, f); }
                                return 1;
                            }
                        };

                        resolve_binary(id, &data_vec);

                        // Notify browser it can release the SHM buffer
                        if let Some(f) = frame {
                            send_shm_free(id, f);
                        }
                    }
                }

                _ => {
                    eprintln!("[IPC ERROR] unexpected message type {} from browser", msg_type);
                }
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
                    let s: CefString = (&v.string_value()).into();
                    let s = s.to_string();
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
                    let s: CefString = (&v.string_value()).into();
                    s.to_string()
                }
                _ => String::new(),
            };

            let context = v8_context_get_current_context().unwrap();
            let promise = v8_value_create_promise().unwrap();

            let id = register_promise(context.clone(), promise.clone());

            debug!("[Renderer] JS invoke: '{}' (id={})", cmd, id);

            if let Some(frame) = context.frame() {
                let mut msg  = process_message_create(Some(&CefString::from("ipc"))).unwrap();
                let msg_args = msg.argument_list().unwrap();

                msg_args.set_int(0, 0);
                msg_args.set_int(1, id as i32);
                msg_args.set_string(2, Some(&CefString::from(cmd.as_str())));
                msg_args.set_string(3, Some(&CefString::from(payload.as_str())));

                frame.send_process_message(ProcessId::BROWSER, Some(&mut msg));
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

            let cmd = match args.get(0) {
                Some(Some(v)) if v.is_string() != 0 => {
                    let s: CefString = (&v.string_value()).into();
                    s.to_string()
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

            let data = unsafe {
                std::slice::from_raw_parts(ptr as *const u8, len)
            };

            let context = v8_context_get_current_context().unwrap();
            let promise = v8_value_create_promise().unwrap();

            let id = register_promise(context.clone(), promise.clone());

            let mut msg = process_message_create(Some(&CefString::from("ipc"))).unwrap();
            let msg_args = msg.argument_list().unwrap();

            msg_args.set_int(0, 3);
            msg_args.set_int(1, id as i32);
            msg_args.set_string(2, Some(&CefString::from(cmd.as_str())));

            if data.len() < SHM_THRESHOLD {
                let mut binary = binary_value_create(Some(data)).unwrap();
                msg_args.set_binary(3, Some(&mut binary));
            } else {

                let mut shm = SharedBuffer::create(data.len());
                shm.write(data);

                let name = shm.name();
                msg_args.set_string(3, Some(&CefString::from(name.as_str())));
                msg_args.set_int(4, data.len() as i32);
                // Keep alive until the response proves browser has read it
                outgoing_shm().lock().unwrap().insert(id, shm);
            }

            if let Some(frame) = context.frame() {
                frame.send_process_message(ProcessId::BROWSER, Some(&mut msg));
            }

            if let Some(ret) = retval {
                *ret = Some(promise);
            }

            1
        }
    }
}
