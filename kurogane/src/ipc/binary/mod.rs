use std::collections::HashMap;

use crate::ipc::browser_state::IpcContext;
use crate::ipc::rpc::PendingMap;

pub type SyncBinaryHandler = Box<dyn Fn(&[u8], IpcContext) -> Result<Vec<u8>, String> + Send + Sync>;
pub type AsyncBinaryHandler = Box<dyn Fn(Vec<u8>, BinaryResponder, IpcContext) + Send + Sync>;

/// Single-use callback for async binary responses.
pub struct BinaryResponder {
    callback: std::sync::Mutex<Option<Box<dyn FnOnce(Result<Vec<u8>, String>, i32) + Send>>>,
}

impl BinaryResponder {
    pub fn new(callback: Box<dyn FnOnce(Result<Vec<u8>, String>, i32) + Send>) -> Self {
        Self {
            callback: std::sync::Mutex::new(Some(callback)),
        }
    }

    pub fn resolve(&self, result: Result<Vec<u8>, String>, error_code: i32) {
        let cb = self.callback.lock().unwrap().take();
        if let Some(cb) = cb {
            cb(result, error_code);
        }
    }
}

pub mod browser;
pub mod renderer;

/// Binary subsystem: handles binary invoke/response message flow.
pub struct BinarySubsystem {
    pub sync_handlers: HashMap<String, SyncBinaryHandler>,
    pub async_handlers: HashMap<String, AsyncBinaryHandler>,
    pub pending: PendingMap,
}

impl BinarySubsystem {
    pub fn new(
        sync_handlers: HashMap<String, SyncBinaryHandler>,
        async_handlers: HashMap<String, AsyncBinaryHandler>,
    ) -> Self {
        Self {
            sync_handlers,
            async_handlers,
            pending: PendingMap::new(),
        }
    }

    pub fn is_async(&self, command: &str) -> bool {
        self.async_handlers.contains_key(command)
    }

    pub fn is_sync(&self, command: &str) -> bool {
        self.sync_handlers.contains_key(command)
    }
}
