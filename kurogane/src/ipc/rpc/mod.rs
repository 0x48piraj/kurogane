//! JSON-based request/response IPC subsystem.
//!
//! Supports synchronous and asynchronous command handlers, pending
//! request tracking, cancellation and promise resolution.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::browser_info_map::{
    BrowserInfoMap, BrowserInfoMapVisitor, BrowserInfoMapVisitorResult,
};
use crate::browser_registry::BrowserId;
use crate::ipc::browser_state::{IpcContext, IpcResult};

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

/// Pending async entry that can be cancelled via AtomicBool flag.
#[derive(Clone)]
pub struct PendingEntry {
    pub aborted: Arc<AtomicBool>,
}

/// Thread-safe handle to the pending map.
/// Closures can clone this handle and manage pending entries independently.
#[derive(Clone)]
pub struct PendingMap {
    inner: Arc<Mutex<BrowserInfoMap<i32, PendingEntry>>>,
}

impl PendingMap {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(BrowserInfoMap::default())),
        }
    }

    pub fn insert(&self, browser_id: BrowserId, id: i32, entry: PendingEntry) {
        self.inner.lock().unwrap().insert(browser_id, id, entry);
    }

    pub fn remove(&self, browser_id: BrowserId, id: i32) -> Option<PendingEntry> {
        self.inner.lock().unwrap().remove(browser_id, id)
    }

    pub fn cancel(&self, browser_id: BrowserId, id: i32) -> bool {
        if let Some(entry) = self.inner.lock().unwrap().remove(browser_id, id) {
            entry.aborted.store(true, Ordering::SeqCst);
            return true;
        }
        false
    }

    pub fn cancel_all_for_browser(&self, browser_id: BrowserId) -> usize {
        struct CancelAllVisitor {
            count: AtomicUsize,
        }

        impl BrowserInfoMapVisitor<i32, PendingEntry> for CancelAllVisitor {
            fn on_next_info(
                &self,
                _browser_id: BrowserId,
                _key: i32,
                value: &PendingEntry,
            ) -> std::ops::ControlFlow<
                BrowserInfoMapVisitorResult,
                BrowserInfoMapVisitorResult,
            > {
                value.aborted.store(true, Ordering::SeqCst);
                self.count.fetch_add(1, Ordering::Relaxed);
                std::ops::ControlFlow::Continue(BrowserInfoMapVisitorResult::RemoveEntry)
            }
        }

        let visitor = CancelAllVisitor {
            count: AtomicUsize::new(0),
        };
        self.inner
            .lock()
            .unwrap()
            .find_browser_all(browser_id, &visitor);
        visitor.count.load(Ordering::Relaxed)
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
