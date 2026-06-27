//! JSON-based request/response IPC subsystem.
//!
//! Supports synchronous and asynchronous command handlers, pending
//! request tracking, cancellation and promise resolution.

use std::collections::HashMap;
use std::sync::Mutex;

use crate::ipc::browser_state::{IpcContext, IpcResult};
use crate::ipc::pending::PendingMap;

pub type SyncRpcHandler = Box<dyn Fn(&str, IpcContext) -> IpcResult + Send + Sync>;
pub type AsyncRpcHandler = Box<dyn Fn(serde_json::Value, IpcResponder, IpcContext) + Send + Sync>;

/// Single-use callback for async RPC responses.
pub struct IpcResponder {
    callback: Mutex<Option<Box<dyn FnOnce(IpcResult, i32) + Send>>>,
}

impl IpcResponder {
    pub fn new(callback: Box<dyn FnOnce(IpcResult, i32) + Send>) -> Self {
        Self {
            callback: Mutex::new(Some(callback)),
        }
    }

    pub fn resolve(&self, result: IpcResult, error_code: i32) {
        let cb = self.callback.lock().unwrap().take();
        if let Some(cb) = cb {
            cb(result, error_code);
        }
    }
}

pub mod browser;
pub mod renderer;

/// RPC subsystem: handles invoke/resolve/reject/cancel message flow.
pub struct RpcSubsystem {
    pub sync_handlers: HashMap<String, SyncRpcHandler>,
    pub async_handlers: HashMap<String, AsyncRpcHandler>,
    pub pending: PendingMap,
}

impl RpcSubsystem {
    pub fn new(
        sync_handlers: HashMap<String, SyncRpcHandler>,
        async_handlers: HashMap<String, AsyncRpcHandler>,
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
