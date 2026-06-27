//! Pending request tracking shared across IPC subsystems.
//!
//! Provides PendingEntry and PendingMap for tracking cancellable pending async operations.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::browser_info_map::{
    BrowserInfoMap, BrowserInfoMapVisitor, BrowserInfoMapVisitorResult,
};
use crate::browser_registry::BrowserId;

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
