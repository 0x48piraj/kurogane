//! Browser-process IPC dispatch and transaction state.
//!
//! Defines the immutable command dispatcher used by the browser process
//! and the runtime state required for active IPC transactions.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::sync::Mutex;

use crate::browser_registry::BrowserId;
use crate::debug;

// Handler types

pub type IpcResult = Result<String, String>;
pub type IpcHandler = Box<dyn Fn(&str) -> IpcResult + Send + Sync>;
pub type BinaryHandler = Box<dyn Fn(&[u8]) -> Result<Vec<u8>, String> + Send + Sync>;

/// Responder for async JSON IPC handlers.
/// Holds a single-use callback that sends the response to the renderer.
/// Thread-safe: can be sent between threads and called exactly once.
pub struct IpcResponder {
    callback: Mutex<Option<Box<dyn FnOnce(IpcResult) + Send>>>,
}

impl IpcResponder {
    pub fn new(callback: Box<dyn FnOnce(IpcResult) + Send>) -> Self {
        Self {
            callback: Mutex::new(Some(callback)),
        }
    }

    /// Send a response. No-op if already called.
    pub fn resolve(&self, result: IpcResult) {
        let cb = self.callback.lock().unwrap().take();
        if let Some(cb) = cb {
            cb(result);
        }
    }
}

/// Responder for async binary IPC handlers.
pub struct BinaryResponder {
    callback: Mutex<Option<Box<dyn FnOnce(Result<Vec<u8>, String>, i32) + Send>>>,
}

impl BinaryResponder {
    pub fn new(callback: Box<dyn FnOnce(Result<Vec<u8>, String>, i32) + Send>) -> Self {
        Self {
            callback: Mutex::new(Some(callback)),
        }
    }

    /// Send a binary response with an error code (0 = success, -1 = panic, etc.).
    pub fn resolve(&self, result: Result<Vec<u8>, String>, error_code: i32) {
        let cb = self.callback.lock().unwrap().take();
        if let Some(cb) = cb {
            cb(result, error_code);
        }
    }
}

pub type AsyncIpcHandler = Box<dyn Fn(serde_json::Value, IpcResponder) + Send + Sync>;
pub type AsyncBinaryHandler = Box<dyn Fn(Vec<u8>, BinaryResponder) + Send + Sync>;

/// A pending async handler entry that can be cancelled.
pub struct PendingEntry {
    pub browser_id: Option<BrowserId>,
    pub aborted: Arc<AtomicBool>,
}

/// Immutable IPC command router.
/// Resolves incoming IPC command names to their registered JSON
/// and binary handlers during browser-process message dispatch.
pub struct IpcDispatcher {
    handlers: HashMap<String, IpcHandler>,
    binary_handlers: HashMap<String, BinaryHandler>,
    async_handlers: HashMap<String, AsyncIpcHandler>,
    async_binary_handlers: HashMap<String, AsyncBinaryHandler>,
    pending: Mutex<HashMap<i32, PendingEntry>>,
}

/// Contextual information for an IPC dispatch call.
/// Carries browser identity without modifying handler signatures.
pub struct IpcContext {
    pub browser_id: Option<BrowserId>,
    pub frame_id: Option<i64>,
}

impl IpcDispatcher {
    pub fn new(
        handlers: HashMap<String, IpcHandler>,
        binary_handlers: HashMap<String, BinaryHandler>,
    ) -> Self {
        Self {
            handlers,
            binary_handlers,
            async_handlers: HashMap::new(),
            async_binary_handlers: HashMap::new(),
            pending: Mutex::new(HashMap::new()),
        }
    }

    pub fn with_async(
        handlers: HashMap<String, IpcHandler>,
        binary_handlers: HashMap<String, BinaryHandler>,
        async_handlers: HashMap<String, AsyncIpcHandler>,
        async_binary_handlers: HashMap<String, AsyncBinaryHandler>,
    ) -> Self {
        Self {
            handlers,
            binary_handlers,
            async_handlers,
            async_binary_handlers,
            pending: Mutex::new(HashMap::new()),
        }
    }

    pub fn dispatch(&self, command: &str, payload: &str) -> IpcResult {
        match self.handlers.get(command) {
            Some(h) => h(payload),
            None => Err(format!("[IPC] unknown command '{command}'")),
        }
    }

    pub fn dispatch_binary(&self, command: &str, payload: &[u8]) -> Result<Vec<u8>, String> {
        match self.binary_handlers.get(command) {
            Some(h) => h(payload),
            None => Err(format!("[IPC] unknown binary command '{command}'")),
        }
    }

    /// Dispatch a JSON command with browser context.
    /// Currently delegates to dispatch - handlers do not yet receive IpcContext.
    pub fn dispatch_with_context(&self, command: &str, payload: &str, _ctx: IpcContext) -> IpcResult {
        self.dispatch(command, payload)
    }

    /// Dispatch a binary command with browser context.
    /// Currently delegates to dispatch_binary - handlers do not yet receive IpcContext.
    pub fn dispatch_binary_with_context(&self, command: &str, payload: &[u8], _ctx: IpcContext) -> Result<Vec<u8>, String> {
        self.dispatch_binary(command, payload)
    }

    /// Check if a command is registered as an async handler.
    pub fn is_async(&self, command: &str) -> bool {
        self.async_handlers.contains_key(command)
    }

    /// Check if a binary command is registered as an async handler.
    pub fn is_async_binary(&self, command: &str) -> bool {
        self.async_binary_handlers.contains_key(command)
    }

    /// Dispatch an async JSON command. Returns true if handled.
    pub fn dispatch_async(&self, command: &str, payload: &str, responder: IpcResponder) -> bool {
        if let Some(handler) = self.async_handlers.get(command) {
            let value: serde_json::Value = if payload.is_empty() {
                serde_json::Value::Null
            } else {
                match serde_json::from_str(payload) {
                    Ok(v) => v,
                    Err(e) => {
                        responder.resolve(Err(format!("invalid JSON payload: {e}")));
                        return true;
                    }
                }
            };
            handler(value, responder);
            true
        } else {
            false
        }
    }

    /// Dispatch an async binary command. Returns true if handled.
    pub fn dispatch_async_binary(&self, command: &str, payload: &[u8], responder: BinaryResponder) -> bool {
        if let Some(handler) = self.async_binary_handlers.get(command) {
            handler(payload.to_vec(), responder);
            true
        } else {
            false
        }
    }

    // Pending handler registry

    pub fn insert_pending(&self, id: i32, entry: PendingEntry) {
        self.pending.lock().unwrap().insert(id, entry);
    }

    pub fn remove_pending(&self, id: i32) {
        self.pending.lock().unwrap().remove(&id);
    }

    /// Cancel a pending handler by id. Returns true if found.
    pub fn cancel_pending(&self, id: i32) -> bool {
        if let Some(entry) = self.pending.lock().unwrap().remove(&id) {
            entry.aborted.store(true, Ordering::SeqCst);
            debug!("[IPC Browser] canceled pending id={}", id);
            true
        } else {
            false
        }
    }

    /// Cancel all pending handlers for a given browser.
    pub fn cancel_all_for_browser(&self, browser_id: BrowserId) -> usize {
        let mut pending = self.pending.lock().unwrap();
        let to_cancel: Vec<i32> = pending
            .iter()
            .filter(|(_, e)| e.browser_id == Some(browser_id))
            .map(|(id, _)| *id)
            .collect();
        let count = to_cancel.len();
        for id in &to_cancel {
            if let Some(entry) = pending.remove(id) {
                entry.aborted.store(true, Ordering::SeqCst);
            }
        }
        if count > 0 {
            debug!("[IPC Browser] canceled {} pending handlers for browser", count);
        }
        count
    }
}
