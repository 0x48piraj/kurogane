//! Renderer process IPC implementation.
//! Transport uses ProcessMessage "ipc" and ListValue typed args.

use cef::*;
use std::sync::Mutex;
use std::collections::HashMap;

use crate::ipc_shm::{SharedBuffer, SHM_THRESHOLD};

//
// Promise registry: Tracks pending promises awaiting responses from the browser process
//

struct PromiseRegistry {
    next_id: u32,
    pending: HashMap<u32, (V8Context, V8Value)>,
}

impl PromiseRegistry {
    fn new() -> Self {
        Self {
            next_id: 1,
            pending: HashMap::new(),
        }
    }

    fn register(&mut self, context: V8Context, promise: V8Value) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);

        self.pending.insert(id, (context, promise));
        id
    }

    fn resolve_string(&mut self, id: u32, success: bool, result: &str) {

        if let Some((context, promise)) = self.pending.remove(&id) {
            if context.enter() == 0 {
                eprintln!("Failed to enter V8 context for promise resolution");
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
            eprintln!("[IPC WARNING] received response for missing promise id={} (likely page reload)", id);
        }
    }

    fn resolve_binary(&mut self, id: u32, data: &[u8]) {
        if let Some((context, promise)) = self.pending.remove(&id) {

            if context.enter() == 0 {
                return;
            }

            let mut owned = data.to_vec();

            let mut buf = v8_value_create_array_buffer(
                owned.as_mut_ptr() as *mut _,
                owned.len(),
                None
            ).unwrap();

            std::mem::forget(owned);

            promise.resolve_promise(Some(&mut buf));

            context.exit();
        }
    }
}

static PROMISE_REGISTRY: Mutex<Option<PromiseRegistry>> = Mutex::new(None);

fn ensure_registry() {
    let mut g = PROMISE_REGISTRY.lock().unwrap();
    if g.is_none() {
        *g = Some(PromiseRegistry::new());
    }
}

fn register_promise(ctx: V8Context, promise: V8Value) -> u32 {
    PROMISE_REGISTRY
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .register(ctx, promise)
}

fn resolve_string(id: u32, success: bool, payload: &str) {
    PROMISE_REGISTRY
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .resolve_string(id, success, payload)
}

fn resolve_binary(id: u32, payload: &[u8]) {
    PROMISE_REGISTRY
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .resolve_binary(id, payload)
}

fn clear_context_promises(ctx: &V8Context) {
    let mut guard = PROMISE_REGISTRY.lock().unwrap();
    let registry = guard.as_mut().unwrap();

    registry.pending.retain(|_, (stored_ctx, _)| {
        let mut other = ctx.clone();
        stored_ctx.is_same(Some(&mut other)) == 0
    });

    println!("[IPC] cleared promises for destroyed JS context");
}

//
// Helpers
//

fn list_int(args: &ListValue, idx: usize) -> i32 {
    args.int(idx)
}

fn list_string(args: &ListValue, idx: usize) -> String {
    let s = args.string(idx);
    let s: CefString = (&s).into();
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

            //
            // JSON invoke
            //

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

            //
            // Binary invoke
            //

            let mut handler = IpcInvokeBinaryHandler::new();

            let mut invoke_binary = v8_value_create_function(
                Some(&CefString::from("invokeBinary")),
                Some(&mut handler),
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

            println!("[Renderer] Injected window.core.invoke + invokeBinary");
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
            _frame: Option<&mut Frame>,
            source_process: ProcessId,
            message: Option<&mut ProcessMessage>,
        ) -> i32 {
            if source_process != ProcessId::BROWSER {
                return 0;
            }

            let msg = message.unwrap();
            let name: CefString = (&msg.name()).into();
            if name.to_string() != "ipc" { return 0; }

            let args = msg.argument_list().unwrap();

            let msg_type = list_int(&args, 0);
            let id = list_int(&args, 1) as u32;

            match msg_type {
                1 => {
                    let payload = list_string(&args, 2);
                    resolve_string(id, true, &payload);
                }

                2 => {
                    let payload = list_string(&args, 2);
                    resolve_string(id, false, &payload);
                }

                4 => {

                    if let Some(binary) = args.binary(2) {

                        let size = binary.size();
                        let mut buf = vec![0u8; size];

                        binary.data(Some(&mut buf), 0);

                        resolve_binary(id, &buf);
                    } else {

                        let name = list_string(&args, 2);
                        let size = list_int(&args, 3) as usize;

                        let shm = SharedBuffer::open(&name, size);

                        resolve_binary(id, shm.as_slice());
                    }
                }

                _ => {
                    eprintln!("[IPC ERROR] invalid message type {} from browser", msg_type);
                }
            }

            1
        }
    }
}

//
// JSON invoke
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
                Some(a) => a,
                None => return 0,
            };

            if args.is_empty() {
                if let Some(exc) = exception {
                    *exc = CefString::from("invoke requires command");
                }
                return 0;
            }

            // first arg: command string
            let cmd = match args.get(0) {
                Some(Some(v)) if v.is_string() != 0 => {
                    let s = v.string_value();
                    let cef: CefString = (&s).into();
                    cef.to_string()
                }
                _ => {
                    if let Some(exc) = exception {
                        *exc = CefString::from("command must be a string");
                    }
                    return 0;
                }
            };

            if cmd.is_empty() {
                if let Some(exc) = exception {
                    *exc = CefString::from("command cannot be empty");
                }
                return 0;
            }

            // optional payload (string)
            let payload = match args.get(1) {
                Some(Some(v)) if v.is_string() != 0 => {
                    let s = v.string_value();
                    let cef: CefString = (&s).into();
                    cef.to_string()
                }
                _ => String::new(),
            };

            let context = v8_context_get_current_context().unwrap();
            let promise = v8_value_create_promise().unwrap();

            let id = register_promise(context.clone(), promise.clone());

            println!("[Renderer] JS invoke: {} (id={})", cmd, id);

            if let Some(frame) = context.frame() {
                let mut msg = process_message_create(Some(&CefString::from("ipc"))).unwrap();
                let args = msg.argument_list().unwrap();

                args.set_int(0, 0); // invoke
                args.set_int(1, id as i32);
                args.set_string(2, Some(&CefString::from(cmd.as_str())));
                args.set_string(3, Some(&CefString::from(payload.as_str())));

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
// Binary invoke
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
                Some(a) => a,
                None => return 0,
            };

            if args.len() < 2 {
                return 0;
            }

            let cmd = match args.get(0) {
                Some(Some(v)) if v.is_string() != 0 => {
                    let s = v.string_value();
                    let c: CefString = (&s).into();
                    c.to_string()
                }
                _ => {
                    if let Some(exc) = exception {
                        *exc = CefString::from("command must be a string");
                    }
                    return 0;
                }
            };

            let buffer = match args.get(1) {
                Some(Some(v)) if v.is_array_buffer() != 0 => v,
                _ => {
                    if let Some(exc) = exception {
                        *exc = CefString::from("second argument must be an ArrayBuffer");
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

            let mut msg =
                process_message_create(Some(&CefString::from("ipc")))
                    .unwrap();

            let list = msg.argument_list().unwrap();

            list.set_int(0, 3);
            list.set_int(1, id as i32);
            list.set_string(2, Some(&CefString::from(cmd.as_str())));

            if data.len() < SHM_THRESHOLD {

                let mut binary =
                    binary_value_create(Some(data))
                        .unwrap();

                list.set_binary(3, Some(&mut binary));

            } else {

                let mut shm = SharedBuffer::create(data.len());
                shm.write(data);

                let name = shm.name();
                list.set_string(3, Some(&CefString::from(name.as_str())));
                list.set_int(4, data.len() as i32);
            }

            if let Some(frame) = context.frame() {
                frame.send_process_message(
                    ProcessId::BROWSER,
                    Some(&mut msg),
                );
            }

            if let Some(ret) = retval {
                *ret = Some(promise);
            }

            1
        }
    }
}
