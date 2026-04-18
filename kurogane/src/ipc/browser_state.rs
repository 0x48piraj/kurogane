//! Browser process IPC state and dispatcher
//!
//! Manages command registration, pending calls and response SHM buffers.
//! Commands can be registered before runtime boot and will be installed
//! when the browser process initializes.

use cef::*;
use std::collections::HashMap;
use serde_json::Value;
use std::sync::{Arc, Mutex, OnceLock};

use crate::ipc::transport::shm::SharedBuffer;

// HANDLER TYPES

pub type IpcResult = Result<String, String>;
pub type IpcHandler = Box<dyn Fn(&str) -> IpcResult + Send + Sync>;
pub type BinaryHandler = Box<dyn Fn(&[u8]) -> Result<Vec<u8>, String> + Send + Sync>;

/// IPC command dispatcher.
/// Maintains both JSON and binary command handlers.
pub struct IpcDispatcher {
    handlers: HashMap<String, IpcHandler>,
    binary_handlers: HashMap<String, BinaryHandler>,
}

impl IpcDispatcher {
    fn new() -> Self {
        Self {
            handlers: HashMap::new(),
            binary_handlers: HashMap::new(),
        }
    }

    pub fn register(&mut self, command: String, handler: IpcHandler) {
        self.handlers.insert(command, handler);
    }

    pub fn register_binary(&mut self, command: String, handler: BinaryHandler) {
        self.binary_handlers.insert(command, handler);
    }

    pub fn dispatch(&self, command: &str, payload: &str) -> IpcResult {
        match self.handlers.get(command) {
            Some(h) => h(payload),
            None => Err(format!("[IPC] Unknown command '{}'", command)),
        }
    }

    pub fn dispatch_binary(&self, command: &str, payload: &[u8]) -> Result<Vec<u8>, String> {
        match self.binary_handlers.get(command) {
            Some(h) => h(payload),
            None => Err(format!("Unknown binary command '{}'", command)),
        }
    }
}

/// Tracks a pending RPC call awaiting response
pub struct PendingCall {
    pub frame: Frame,
    pub frame_id: String,
}

//
// Global state
//

/// Live dispatcher (exists only after CEF browser process starts)
static DISPATCHER: OnceLock<Arc<Mutex<IpcDispatcher>>> = OnceLock::new();

/// Commands registered before runtime boot
static PENDING_COMMANDS: OnceLock<Mutex<Vec<(String, IpcHandler)>>> = OnceLock::new();

/// Binary commands registered before runtime boot (pending buffer)
static PENDING_BINARY_COMMANDS: OnceLock<Mutex<Vec<(String, BinaryHandler)>>> = OnceLock::new();

/// Tracks pending RPC calls
static PENDING_CALLS: OnceLock<Mutex<HashMap<u32, PendingCall>>> = OnceLock::new();

/// Keep SHM alive until the renderer signals it has finished reading (msg_type 5)
static RESPONSE_SHM_STORE: OnceLock<Mutex<HashMap<u32, SharedBuffer>>> = OnceLock::new();

//
// State accessors
//

pub fn pending_calls() -> &'static Mutex<HashMap<u32, PendingCall>> {
    PENDING_CALLS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn pending_commands() -> &'static Mutex<Vec<(String, IpcHandler)>> {
    PENDING_COMMANDS.get_or_init(|| Mutex::new(Vec::new()))
}

pub fn pending_binary_commands() -> &'static Mutex<Vec<(String, BinaryHandler)>> {
    PENDING_BINARY_COMMANDS.get_or_init(|| Mutex::new(Vec::new()))
}

pub fn response_shm_store() -> &'static Mutex<HashMap<u32, SharedBuffer>> {
    RESPONSE_SHM_STORE.get_or_init(|| Mutex::new(HashMap::new()))
}

//
// Dispatcher initialization
//

/// Dispatcher init: Called by runtime when browser process initializes.
/// Drains both JSON and binary pending command queues.
pub fn init_dispatcher() -> Arc<Mutex<IpcDispatcher>> {
    let dispatcher = DISPATCHER
        .get_or_init(|| Arc::new(Mutex::new(IpcDispatcher::new())))
        .clone();

    {
        let mut pending = pending_commands().lock().unwrap();
        let mut pending_bin = pending_binary_commands().lock().unwrap();
        let mut disp = dispatcher.lock().unwrap();

        for (cmd, handler) in pending.drain(..) {
            disp.register(cmd, handler);
        }

        for (cmd, handler) in pending_bin.drain(..) {
            disp.register_binary(cmd, handler);
        }
    }

    dispatcher
}

/// Get dispatcher after initialization
pub fn get_dispatcher() -> Arc<Mutex<IpcDispatcher>> {
    DISPATCHER
        .get()
        .expect("IPC dispatcher not initialized")
        .clone()
}

//
// Public registration APIs
//

/// Register a JSON command. Safe to call before runtime boot.
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
/// Register a binary command. Safe to call before runtime boot.
pub fn register_binary_command<F>(command: impl Into<String>, handler: F)
where
    F: Fn(&[u8]) -> Result<Vec<u8>, String> + Send + Sync + 'static,
{
    let wrapped: BinaryHandler = Box::new(handler);

    if let Some(dispatcher) = DISPATCHER.get() {
        dispatcher.lock().unwrap().register_binary(command.into(), wrapped);
    } else {
        pending_binary_commands().lock().unwrap().push((command.into(), wrapped));
    }
}
