//! Browser process IPC dispatcher.
//! Commands may be registered before CEF starts.
//! They are buffered and installed once the browser process initializes.
//! Exposes JSON API while transport remains string based.

use cef::*;
use std::sync::{Arc, Mutex, OnceLock};
use std::collections::HashMap;
use serde_json::Value;

use crate::ipc_shm::{SharedBuffer, SHM_THRESHOLD};
use crate::debug;

pub type IpcResult = Result<String, String>;
pub type IpcHandler = Box<dyn Fn(&str) -> IpcResult + Send + Sync>;

pub type BinaryHandler =
    Box<dyn Fn(&[u8]) -> Result<Vec<u8>, String> + Send + Sync>;

pub struct IpcDispatcher {
    handlers: HashMap<String, IpcHandler>,
    binary_handlers: HashMap<String, BinaryHandler>,
}

struct PendingCall {
    frame: Frame,
    frame_id: String,
}

impl IpcDispatcher {
    fn new() -> Self {
        Self {
            handlers: HashMap::new(),
            binary_handlers: HashMap::new(),
        }
    }

    pub fn register(&mut self, command: impl Into<String>, handler: IpcHandler) {
        self.handlers.insert(command.into(), handler);
    }

    pub fn register_binary(
        &mut self,
        command: impl Into<String>,
        handler: BinaryHandler,
    ) {
        self.binary_handlers.insert(command.into(), handler);
    }

    fn dispatch(&self, command: &str, payload: &str) -> IpcResult {
        if let Some(handler) = self.handlers.get(command) {
            handler(payload)
        } else {
            Err(format!("[IPC] Unknown command '{}'", command))
        }
    }

    fn dispatch_binary(
        &self,
        command: &str,
        payload: &[u8],
    ) -> Result<Vec<u8>, String> {
        if let Some(handler) = self.binary_handlers.get(command) {
            handler(payload)
        } else {
            Err(format!("Unknown binary command '{}'", command))
        }
    }
}

//
// Global state
//

/// Live dispatcher (exists only after CEF browser process starts)
static DISPATCHER: OnceLock<Arc<Mutex<IpcDispatcher>>> = OnceLock::new();

/// Commands registered before runtime boot
static PENDING_COMMANDS: OnceLock<Mutex<Vec<(String, IpcHandler)>>> = OnceLock::new();

static PENDING_CALLS: OnceLock<Mutex<HashMap<u32, PendingCall>>> = OnceLock::new();

fn pending_calls() -> &'static Mutex<HashMap<u32, PendingCall>> {
    PENDING_CALLS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn pending_commands() -> &'static Mutex<Vec<(String, IpcHandler)>> {
    PENDING_COMMANDS.get_or_init(|| Mutex::new(Vec::new()))
}

/// Dispatcher init: Called by runtime when browser process initializes.
/// Drains any commands registered before init.
pub fn init_dispatcher() -> Arc<Mutex<IpcDispatcher>> {
    let dispatcher = DISPATCHER
        .get_or_init(|| Arc::new(Mutex::new(IpcDispatcher::new())))
        .clone();

    {
        let mut pending = pending_commands().lock().unwrap();
        let mut disp = dispatcher.lock().unwrap();

        for (cmd, handler) in pending.drain(..) {
            disp.register(cmd, handler);
        }
    }

    dispatcher
}

/// Get dispatcher after init
pub fn get_dispatcher() -> Arc<Mutex<IpcDispatcher>> {
    DISPATCHER
        .get()
        .expect("IPC dispatcher not initialized")
        .clone()
}

/// Public JSON API
pub fn register_command<F>(command: impl Into<String>, handler: F)
where
    F: Fn(Value) -> Result<Value, String> + Send + Sync + 'static,
{
    let wrapped: IpcHandler = Box::new(move |payload: &str| {
        let input: Value =
            serde_json::from_str(payload).unwrap_or(Value::String(payload.to_string()));

        match handler(input) {
            Ok(v) => Ok(serde_json::to_string(&v).unwrap()),
            Err(e) => Err(e),
        }
    });

    if let Some(dispatcher) = DISPATCHER.get() {
        dispatcher.lock().unwrap().register(command.into(), wrapped);
    } else {
        pending_commands().lock().unwrap().push((command.into(), wrapped));
    }
}

//
// Binary API
//

pub fn register_binary_command<F>(
    command: impl Into<String>,
    handler: F,
)
where
    F: Fn(&[u8]) -> Result<Vec<u8>, String> + Send + Sync + 'static,
{
    let wrapped: BinaryHandler = Box::new(handler);

    if let Some(dispatcher) = DISPATCHER.get() {
        dispatcher.lock().unwrap().register_binary(command.into(), wrapped);
    }
}

//
// Message helpers
//

fn list_get_int(args: &ListValue, idx: usize) -> i32 {
    // binding exposes .int(index)
    args.int(idx)
}

fn list_get_string(args: &ListValue, idx: usize) -> String {
    // binding exposes .string(index) -> CefStringUserfree
    let userfree = args.string(idx);
    // Convert to CefString (borrow conversion) then to Rust String
    let cef: CefString = (&userfree).into();
    cef.to_string()
}

//
// IPC message handling
//

pub fn handle_ipc_message(
    _browser: &mut Browser,
    frame: &mut Frame,
    message: &mut ProcessMessage,
) -> bool {
    let name: CefString = (&message.name()).into();
    if name.to_string() != "ipc" {
        return false;
    }

    let args = match message.argument_list() {
        Some(a) => a,
        None => return false,
    };

    // Message type: 0 = invoke, 1 = resolve (browser shouldn't receive), 2 = reject
    let msg_type = list_get_int(&args, 0);

    match msg_type {

        // JSON invoke
        0 => {
            let id = list_get_int(&args, 1) as u32;
            let command = list_get_string(&args, 2);
            let payload = list_get_string(&args, 3);

            debug!("[Browser] IPC invoke: '{}' (id={})", command, id);

            let dispatcher = get_dispatcher();
            let result = dispatcher.lock().unwrap().dispatch(&command, &payload);

            let frame_id = {
                let s: CefString = (&frame.identifier()).into();
                s.to_string()
            };

            pending_calls().lock().unwrap().insert(
                id,
                PendingCall {
                    frame: frame.clone(),
                    frame_id,
                },
            );

            send_response(id, result);
            true
        }

        // Binary invoke
        3 => {
            let id = list_get_int(&args, 1) as u32;
            let command = list_get_string(&args, 2);

            let data: Vec<u8>;

            if let Some(binary) = args.binary(3) {

                let size = binary.size();
                let mut buf = vec![0u8; size];

                binary.data(Some(&mut buf), 0);

                data = buf;
            } else {
                let name = list_get_string(&args, 3);
                let size = list_get_int(&args, 4) as usize;

                let shm = SharedBuffer::open(&name, size);
                data = shm.as_slice().to_vec();
            }

            let dispatcher = get_dispatcher();

            let result = dispatcher
                .lock()
                .unwrap()
                .dispatch_binary(&command, &data);

            send_binary_response(id, result, frame);

            true
        }

        _ => false,
    }
}

//
// JSON response
//

fn send_response(id: u32, result: IpcResult)
{
    let call = {
        let mut map = pending_calls().lock().unwrap();
        map.remove(&id)
    };

    let Some(call) = call else {
        debug!("[IPC] dropping response {}, caller gone", id);
        return;
    };

    // frame no longer exists
    if call.frame.is_valid() == 0 {
        debug!("[IPC] frame destroyed, dropping {}", id);
        return;
    }

    // navigation changed frame identity
    let current_id = {
        let s: CefString = (&call.frame.identifier()).into();
        s.to_string()
    };

    if current_id != call.frame_id {
        debug!("[IPC] navigation changed frame, dropping stale response {}", id);
        return;
    }

    let mut msg = process_message_create(Some(&CefString::from("ipc"))).unwrap();
    let args = msg.argument_list().unwrap();

    match result {
        Ok(payload) => {
            args.set_int(0, 1); // resolve
            args.set_int(1, id as i32);
            args.set_string(2, Some(&CefString::from(payload.as_str())));
        }

        Err(err) => {
            args.set_int(0, 2); // reject
            args.set_int(1, id as i32);
            args.set_string(2, Some(&CefString::from(err.as_str())));
        }
    }

    call.frame.send_process_message(ProcessId::RENDERER, Some(&mut msg));
}

//
// Binary response
//

fn send_binary_response(
    id: u32,
    result: Result<Vec<u8>, String>,
    frame: &Frame,
) {
    let mut msg =
        process_message_create(Some(&CefString::from("ipc"))).unwrap();

    let args = msg.argument_list().unwrap();

    match result {

        Ok(data) => {
            args.set_int(0, 4);
            args.set_int(1, id as i32);

            if data.len() < SHM_THRESHOLD {

                let mut binary =
                    binary_value_create(Some(data.as_slice())).unwrap();

                args.set_binary(2, Some(&mut binary));

            } else {

                let mut shm = SharedBuffer::create(data.len());
                shm.write(&data);

                let name = shm.name();
                args.set_string(2, Some(&CefString::from(name.as_str())));
                args.set_int(3, data.len() as i32);
            }
        }

        Err(err) => {
            args.set_int(0, 2); // reject
            args.set_int(1, id as i32);
            args.set_string(2, Some(&CefString::from(err.as_str())));
        }
    }

    frame.send_process_message(ProcessId::RENDERER, Some(&mut msg));
}
