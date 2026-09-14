//! Pending request tracking for async request/response IPC.
//!
//! Entries are keyed by browser, frame, origin and correlation id.
//! Correlation ids are allocated per renderer process and a cross-origin
//! iframe runs in its own renderer, so two frames of one browser can use the
//! same id.
//!
//! Requests may be cancelled only by their originating frame and origin.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::acl::Origin;
use crate::browser_registry::BrowserId;
use crate::ipc::FrameId;

/// Pending async entry that can be cancelled via its abort flag.
#[derive(Clone)]
pub struct PendingEntry {
    pub aborted: Arc<AtomicBool>, // Coordination mechanism between cancel and resolve
}

/// Identifies one pending request.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct PendingKey {
    browser: BrowserId,
    frame: FrameId,
    origin: Origin,
    id: i32,
}

impl PendingKey {
    pub fn new(browser: BrowserId, frame: FrameId, origin: Origin, id: i32) -> Self {
        Self {
            browser,
            frame,
            origin,
            id,
        }
    }
}

/// Thread-safe handle to the pending map. Clones share one map.
#[derive(Clone, Default)]
pub struct PendingMap {
    inner: Arc<Mutex<HashMap<PendingKey, PendingEntry>>>,
}

impl PendingMap {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&self, key: PendingKey, entry: PendingEntry) {
        self.inner.lock().unwrap().insert(key, entry);
    }

    pub fn remove(&self, key: &PendingKey) -> Option<PendingEntry> {
        self.inner.lock().unwrap().remove(key)
    }

    /// Aborts and removes the entry; false when there was none.
    pub fn cancel(&self, key: &PendingKey) -> bool {
        match self.inner.lock().unwrap().remove(key) {
            Some(entry) => {
                entry.aborted.store(true, Ordering::SeqCst);
                true
            }
            None => false,
        }
    }

    /// Aborts and removes every entry of `browser_id`; returns how many.
    pub fn cancel_all_for_browser(&self, browser_id: BrowserId) -> usize {
        self.cancel_where(|key| key.browser == browser_id)
    }

    /// Aborts and removes every entry `frame` sent; returns how many.
    pub fn cancel_frame(&self, frame: &FrameId) -> usize {
        self.cancel_where(|key| key.frame == *frame)
    }

    fn cancel_where(&self, mut matches: impl FnMut(&PendingKey) -> bool) -> usize {
        let mut cancelled = 0;
        self.inner.lock().unwrap().retain(|key, entry| {
            if !matches(key) {
                return true;
            }
            entry.aborted.store(true, Ordering::SeqCst);
            cancelled += 1;
            false
        });
        cancelled
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::responder::Responder;
    use std::sync::atomic::AtomicUsize;

    fn key(browser: u32, frame: &str, id: i32) -> PendingKey {
        keyed(browser, frame, "app://app", id)
    }

    fn keyed(browser: u32, frame: &str, origin: &str, id: i32) -> PendingKey {
        let origin = Origin::parse(origin).unwrap();
        PendingKey::new(BrowserId::new(browser), FrameId::new(frame), origin, id)
    }

    fn entry() -> PendingEntry {
        PendingEntry {
            aborted: Arc::new(AtomicBool::new(false)),
        }
    }

    #[test]
    fn entries_are_found_only_by_their_exact_key() {
        let map = PendingMap::new();
        map.insert(key(1, "f", 10), entry());
        assert!(map.remove(&key(2, "f", 10)).is_none(), "other browser");
        assert!(map.remove(&key(1, "g", 10)).is_none(), "other frame");
        assert!(map.remove(&key(1, "f", 99)).is_none(), "other id");
        assert!(map.remove(&key(1, "f", 10)).is_some());
        assert!(map.remove(&key(1, "f", 10)).is_none(), "removed once");
    }

    #[test]
    fn frames_with_the_same_correlation_id_do_not_collide() {
        let map = PendingMap::new();
        let main = entry();
        let main_flag = main.aborted.clone();
        map.insert(key(1, "main", 1), main);
        map.insert(key(1, "iframe", 1), entry());

        assert!(map.cancel(&key(1, "iframe", 1)));
        assert!(
            !main_flag.load(Ordering::SeqCst),
            "the iframe cannot cancel the main frame"
        );
        assert!(map.remove(&key(1, "main", 1)).is_some());
    }

    #[test]
    fn a_later_origin_in_the_same_frame_cannot_cancel() {
        let map = PendingMap::new();
        let e = entry();
        let flag = e.aborted.clone();
        map.insert(keyed(1, "f", "app://app", 1), e);
        assert!(!map.cancel(&keyed(1, "f", "https://evil.example", 1)));
        assert!(!flag.load(Ordering::SeqCst));
        assert!(map.cancel(&keyed(1, "f", "app://app", 1)));
    }

    #[test]
    fn cancel_frame_touches_only_that_frame() {
        let map = PendingMap::new();
        let (first, second, other) = (entry(), entry(), entry());
        let flags = [
            first.aborted.clone(),
            second.aborted.clone(),
            other.aborted.clone(),
        ];
        map.insert(key(1, "f", 1), first);
        map.insert(keyed(1, "f", "https://other.example", 2), second);
        map.insert(key(1, "g", 1), other);

        assert_eq!(
            map.cancel_frame(&FrameId::new("f")),
            2,
            "every origin the frame showed"
        );
        assert!(flags[0].load(Ordering::SeqCst) && flags[1].load(Ordering::SeqCst));
        assert!(!flags[2].load(Ordering::SeqCst));
        assert!(map.remove(&key(1, "g", 1)).is_some());
    }

    #[test]
    fn cancel_sets_the_flag_and_removes_the_entry_once() {
        let map = PendingMap::new();
        let e = entry();
        let flag = e.aborted.clone();
        map.insert(key(1, "f", 0), e);
        assert!(map.cancel(&key(1, "f", 0)));
        assert!(flag.load(Ordering::SeqCst));
        assert!(!map.cancel(&key(1, "f", 0)));
        assert!(map.remove(&key(1, "f", 0)).is_none());
    }

    #[test]
    fn cancel_all_for_browser_touches_only_that_browser() {
        let map = PendingMap::new();
        let other = entry();
        let other_flag = other.aborted.clone();
        map.insert(key(1, "a", 1), entry());
        map.insert(key(1, "b", 2), entry());
        map.insert(key(2, "a", 1), other);

        assert_eq!(map.cancel_all_for_browser(BrowserId::new(1)), 2);
        assert_eq!(map.cancel_all_for_browser(BrowserId::new(1)), 0);
        assert!(!other_flag.load(Ordering::SeqCst));
        assert!(map.remove(&key(2, "a", 1)).is_some());
    }

    #[test]
    fn clones_share_state() {
        let a = PendingMap::new();
        let b = a.clone();
        a.insert(key(1, "f", 10), entry());
        assert!(b.remove(&key(1, "f", 10)).is_some());
    }

    #[test]
    fn concurrent_insert_cancel_and_cancel_all_are_safe() {
        let map = PendingMap::new();
        let handles: Vec<_> = (0..8)
            .map(|t| {
                let map = map.clone();
                std::thread::spawn(move || {
                    for i in 0..100 {
                        map.insert(key(t % 2, "f", i), entry());
                        map.cancel(&key(t % 2, "f", i));
                        if i % 25 == 0 {
                            map.cancel_all_for_browser(BrowserId::new(t % 2));
                        }
                    }
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }
        map.cancel_all_for_browser(BrowserId::new(0));
        map.cancel_all_for_browser(BrowserId::new(1));
        assert!(map.remove(&key(0, "f", 0)).is_none());
    }

    #[test]
    fn cancellation_suppresses_late_responses() {
        let calls = Arc::new(AtomicUsize::new(0));
        let map = PendingMap::new();
        let mut responders = Vec::new();
        for id in 0..10 {
            let e = entry();
            let calls = calls.clone();
            responders.push(Responder::with_abort(
                Box::new(move |_: Result<(), _>| {
                    calls.fetch_add(1, Ordering::SeqCst);
                }),
                e.aborted.clone(),
            ));
            map.insert(key(1, "f", id), e);
        }
        assert!(map.cancel(&key(1, "f", 0)));
        assert_eq!(map.cancel_all_for_browser(BrowserId::new(1)), 9);
        for responder in responders {
            responder.resolve(Ok(()));
        }
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }
}
