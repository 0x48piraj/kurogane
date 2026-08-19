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
    pub aborted: Arc<AtomicBool>, // Coordination mechanism between cancel and resolve
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

impl Default for PendingMap {
    fn default() -> Self {
        Self::new()
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser_registry::BrowserId;
    use crate::ipc::responder::Responder;

    fn bid(id: u32) -> BrowserId {
        BrowserId::new(id)
    }

    fn make_entry() -> PendingEntry {
        PendingEntry {
            aborted: Arc::new(AtomicBool::new(false)),
        }
    }

    // An inserted entry can be removed using its browser and request ID
    #[test]
    fn insert_and_remove_returns_entry() {
        let map = PendingMap::new();
        let entry = make_entry();
        map.insert(bid(1), 10, entry);
        let removed = map.remove(bid(1), 10);
        assert!(removed.is_some(), "remove should return the entry");
    }

    // Removing a missing entry returns None
    #[test]
    fn remove_nonexistent_returns_none() {
        let map = PendingMap::new();
        assert!(map.remove(bid(1), 10).is_none());
    }

    // Entries belonging to another browser are not accessible
    #[test]
    fn remove_wrong_browser_returns_none() {
        let map = PendingMap::new();
        map.insert(bid(1), 10, make_entry());
        assert!(map.remove(bid(2), 10).is_none(), "wrong browser should not find entry");
        assert!(map.remove(bid(1), 10).is_some(), "correct browser should find entry");
    }

    // A request can only be removed using its exact request ID
    #[test]
    fn remove_wrong_id_returns_none() {
        let map = PendingMap::new();
        map.insert(bid(1), 10, make_entry());
        assert!(map.remove(bid(1), 99).is_none());
    }

    // Removing an entry twice succeeds only once
    #[test]
    fn remove_is_idempotent() {
        let map = PendingMap::new();
        map.insert(bid(1), 10, make_entry());
        assert!(map.remove(bid(1), 10).is_some());
        assert!(map.remove(bid(1), 10).is_none());
    }

    // Inserting an existing request ID replaces the previous entry
    #[test]
    fn insert_duplicate_id_overwrites() {
        let map = PendingMap::new();
        let e1 = make_entry();
        let e2 = make_entry();
        map.insert(bid(1), 10, e1);
        map.insert(bid(1), 10, e2);
        let removed = map.remove(bid(1), 10).unwrap();
        assert!(!removed.aborted.load(Ordering::SeqCst));
    }

    // Cancelling a request marks its shared cancellation flag
    #[test]
    fn cancel_sets_aborted_flag() {
        let map = PendingMap::new();
        let entry = make_entry();
        let flag = entry.aborted.clone();
        map.insert(bid(1), 10, entry);
        let cancelled = map.cancel(bid(1), 10);
        assert!(cancelled, "cancel should return true for existing entry");
        assert!(flag.load(Ordering::SeqCst), "aborted flag must be true after cancel");
    }

    // Cancelling a request removes it from the pending map
    #[test]
    fn cancel_removes_entry() {
        let map = PendingMap::new();
        map.insert(bid(1), 10, make_entry());
        map.cancel(bid(1), 10);
        assert!(map.remove(bid(1), 10).is_none(), "entry should be gone after cancel");
    }

    // Cancelling a missing request reports that nothing was cancelled
    #[test]
    fn cancel_nonexistent_returns_false() {
        let map = PendingMap::new();
        assert!(!map.cancel(bid(1), 10));
    }

    // Cancelling the same request twice reports success only once
    #[test]
    fn cancel_idempotent() {
        let map = PendingMap::new();
        map.insert(bid(1), 10, make_entry());
        assert!(map.cancel(bid(1), 10));
        assert!(!map.cancel(bid(1), 10));
    }

    // Cancelling a request does not affect requests belonging to another browser
    #[test]
    fn cancel_does_not_affect_other_browsers() {
        let map = PendingMap::new();
        map.insert(bid(1), 10, make_entry());
        let entry_other = make_entry();
        let flag_other = entry_other.aborted.clone();
        map.insert(bid(2), 10, entry_other);
        map.cancel(bid(1), 10);
        assert!(!flag_other.load(Ordering::SeqCst), "bid(2) entry must not be aborted");
        assert!(map.remove(bid(2), 10).is_some(), "bid(2) entry should still exist");
    }

    // Cancelling one request does not affect other request IDs for the same browser
    #[test]
    fn cancel_does_not_affect_other_ids() {
        let map = PendingMap::new();
        map.insert(bid(1), 10, make_entry());
        map.insert(bid(1), 20, make_entry());
        map.cancel(bid(1), 10);
        assert!(map.remove(bid(1), 20).is_some(), "id 20 should still exist");
    }

    // Cancelling a browser marks every pending request for that browser as aborted
    #[test]
    fn cancel_all_for_browser_sets_all_aborted() {
        let map = PendingMap::new();
        let e1 = make_entry();
        let e2 = make_entry();
        let e3 = make_entry();
        let f1 = e1.aborted.clone();
        let f2 = e2.aborted.clone();
        let f3 = e3.aborted.clone();
        map.insert(bid(1), 1, e1);
        map.insert(bid(1), 2, e2);
        map.insert(bid(1), 3, e3);

        let count = map.cancel_all_for_browser(bid(1));
        assert_eq!(count, 3, "should report 3 cancelled entries");
        assert!(f1.load(Ordering::SeqCst));
        assert!(f2.load(Ordering::SeqCst));
        assert!(f3.load(Ordering::SeqCst));
    }

    // Cancelling a browser removes all of its pending requests
    #[test]
    fn cancel_all_for_browser_removes_entries() {
        let map = PendingMap::new();
        map.insert(bid(1), 1, make_entry());
        map.insert(bid(1), 2, make_entry());
        map.insert(bid(1), 3, make_entry());
        map.cancel_all_for_browser(bid(1));
        assert!(map.remove(bid(1), 1).is_none());
        assert!(map.remove(bid(1), 2).is_none());
        assert!(map.remove(bid(1), 3).is_none());
    }

    // Cancelling one browser does not affect pending requests for another browser
    #[test]
    fn cancel_all_for_browser_does_not_affect_others() {
        let map = PendingMap::new();
        let entry_other = make_entry();
        let flag_other = entry_other.aborted.clone();
        map.insert(bid(1), 1, make_entry());
        map.insert(bid(2), 1, entry_other);

        map.cancel_all_for_browser(bid(1));
        assert!(!flag_other.load(Ordering::SeqCst), "bid(2) entry must not be aborted");
        assert!(map.remove(bid(2), 1).is_some(), "bid(2) entry must still exist");
    }

    // Cancelling a browser with no pending requests returns zero
    #[test]
    fn cancel_all_for_nonexistent_browser_returns_zero() {
        let map = PendingMap::new();
        map.insert(bid(1), 1, make_entry());
        assert_eq!(map.cancel_all_for_browser(bid(99)), 0);
    }

    // Cancelling an empty map returns zero
    #[test]
    fn cancel_all_for_empty_map_returns_zero() {
        let map = PendingMap::new();
        assert_eq!(map.cancel_all_for_browser(bid(1)), 0);
    }

    // Repeating browser-wide cancellation reports no additional cancellations
    #[test]
    fn cancel_all_idempotent() {
        let map = PendingMap::new();
        map.insert(bid(1), 1, make_entry());
        map.insert(bid(1), 2, make_entry());
        assert_eq!(map.cancel_all_for_browser(bid(1)), 2);
        assert_eq!(map.cancel_all_for_browser(bid(1)), 0);
    }

    // Requests with the same ID remain independent across browsers
    #[test]
    fn entries_for_different_browsers_are_isolated() {
        let map = PendingMap::new();
        map.insert(bid(1), 10, make_entry());
        map.insert(bid(2), 10, make_entry());
        assert!(map.remove(bid(1), 10).is_some());
        assert!(map.remove(bid(2), 10).is_some(), "bid(2) entry still exists");
    }

    // Browser-wide cancellation only affects the targeted browser
    #[test]
    fn multiple_browsers_independent_cancel_all() {
        let map = PendingMap::new();
        map.insert(bid(1), 1, make_entry());
        map.insert(bid(2), 1, make_entry());
        map.insert(bid(2), 2, make_entry());

        assert_eq!(map.cancel_all_for_browser(bid(1)), 1);
        assert!(map.remove(bid(2), 1).is_some());
        assert!(map.remove(bid(2), 2).is_some());
    }

    // Cloned PendingMap handles share the same underlying pending state
    #[test]
    fn pending_map_clone_shares_state() {
        let map1 = PendingMap::new();
        let map2 = map1.clone();
        map1.insert(bid(1), 10, make_entry());
        assert!(map2.remove(bid(1), 10).is_some(), "clone should see entries from original");
    }

    // Concurrent inserts remain consistent and all inserted entries remain removable
    #[test]
    fn concurrent_insert_and_remove_is_safe() {
        use std::thread;

        let map = Arc::new(PendingMap::new());
        let mut handles = Vec::new();

        // Spawn inserters
        for i in 0..10 {
            let map = map.clone();
            handles.push(thread::spawn(move || {
                map.insert(bid(1), i, make_entry());
            }));
        }

        for handle in handles {
            handle.join().unwrap();
        }

        let mut found = 0;
        for i in 0..10 {
            if map.remove(bid(1), i).is_some() {
                found += 1;
            }
        }
        assert_eq!(found, 10, "all 10 inserted entries must be removable");
    }

    // Concurrent insertion and cancellation remain safe
    #[test]
    fn concurrent_cancel_and_insert_is_safe() {
        use std::thread;

        let map = Arc::new(PendingMap::new());
        let mut handles = Vec::new();

        // Thread 1: insert ids 0..100
        let inserter = map.clone();
        handles.push(thread::spawn(move || {
            for i in 0..100 {
                inserter.insert(bid(1), i, make_entry());
            }
        }));

        // Thread 2: cancel ids 0..100 (may or may not find them)
        let canceller = map.clone();
        handles.push(thread::spawn(move || {
            for i in 0..100 {
                canceller.cancel(bid(1), i);
            }
        }));

        for handle in handles {
            handle.join().unwrap(); // should not panic
        }
    }

    // Concurrent browser-wide cancellation remains safe
    #[test]
    fn concurrent_cancel_all_is_safe() {
        use std::thread;

        let map = Arc::new(PendingMap::new());

        // Pre-populate
        for browser in 0..5 {
            for id in 0..20 {
                map.insert(bid(browser), id, make_entry());
            }
        }

        let mut handles = Vec::new();
        for browser in 0..5 {
            let map = map.clone();
            handles.push(thread::spawn(move || {
                map.cancel_all_for_browser(bid(browser));
            }));
        }

        for handle in handles {
            handle.join().unwrap();
        }

        for browser in 0..5 {
            for id in 0..20 {
                assert!(
                    map.remove(bid(browser), id).is_none(),
                    "all entries for browser {} should be cancelled/removed",
                    browser
                );
            }
        }
    }

    // Resolving a request before cancellation makes a later cancellation a no-op
    #[test]
    fn resolve_then_cancel_is_benign() {
        let map = PendingMap::new();
        let entry = make_entry();
        map.insert(bid(1), 10, entry);

        // Simulate resolve
        let removed = map.remove(bid(1), 10);
        assert!(removed.is_some());

        // Cancel after resolve
        let cancelled = map.cancel(bid(1), 10);
        assert!(!cancelled, "cancel after resolve should return false");
    }

    // Concurrent insertion and cancellation remain safe for the same request IDs
    #[test]
    fn cancel_during_concurrent_insert_same_id() {
        // Cancel and insert on the same (browser_id, id) from
        // different threads must not deadlock or corrupt the map
        let map = PendingMap::new();
        let mut handles = Vec::new();

        for _ in 0..10 {
            let map = map.clone();
            handles.push(std::thread::spawn(move || {
                for i in 0..100 {
                    map.insert(bid(1), i, make_entry());
                    map.cancel(bid(1), i);
                }
            }));
        }

        for handle in handles {
            handle.join().unwrap();
        }

        // Map should be clean, all entries were cancelled
        for i in 0..100 {
            assert!(map.remove(bid(1), i).is_none(),
                "entry {i} should be removed after concurrent cancel");
        }
    }

    // Request ID zero is treated as a valid pending request identifier
    #[test]
    fn insert_id_zero_works() {
        let map = PendingMap::new();
        let entry = make_entry();
        let flag = entry.aborted.clone();
        map.insert(bid(1), 0, entry);
        assert!(map.cancel(bid(1), 0));
        assert!(flag.load(Ordering::SeqCst));
    }

    // Browser-wide cancellation remains safe while concurrent requests are being inserted
    #[test]
    fn cancel_all_for_browser_during_insert() {
        let map = PendingMap::new();
        let mut handles = Vec::new();

        // Inserter thread
        let inserter = map.clone();
        handles.push(std::thread::spawn(move || {
            for i in 0..200 {
                inserter.insert(bid(1), i, make_entry());
            }
        }));

        // Canceller thread
        let canceller = map.clone();
        handles.push(std::thread::spawn(move || {
            for _ in 0..50 {
                canceller.cancel_all_for_browser(bid(1));
            }
        }));

        for handle in handles {
            handle.join().unwrap();
        }
    }

    // Cancellation through PendingMap prevents a late responder from delivering
    #[test]
    fn cancel_then_resolve_suppresses_callback() {
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_clone = calls.clone();

        let map = PendingMap::new();
        let entry = make_entry();
        let responder = Responder::with_abort(
            Box::new(move |_| {
                calls_clone.fetch_add(1, Ordering::SeqCst);
            }),
            entry.aborted.clone(),
        );

        map.insert(bid(1), 10, entry);

        // Cancel wins
        assert!(map.cancel(bid(1), 10));

        // Late resolve
        responder.resolve(Ok(())); // callback must not fire

        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    // Browser-wide cancellation prevents all pending responders from delivering late responses
    #[test]
    fn cancel_all_for_browser_suppresses_late_responses() {
        let calls = Arc::new(AtomicUsize::new(0));
        let mut responders = Vec::new();
        let map = PendingMap::new();

        for id in 0..50 {
            let entry = make_entry();
            let calls = calls.clone();

            let responder = Responder::with_abort(
                Box::new(move |_| {
                    calls.fetch_add(1, Ordering::SeqCst);
                }),
                entry.aborted.clone(),
            );

            map.insert(bid(1), id, entry);
            responders.push(responder);
        }

        // Browser teardown
        assert_eq!(map.cancel_all_for_browser(bid(1)), 50);

        // Late responses from all 50 handlers
        for responder in responders { // none should deliver
            responder.resolve(Ok(()));
        }

        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }
}
