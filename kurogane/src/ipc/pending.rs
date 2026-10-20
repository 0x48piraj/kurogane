//! Pending request state for async request/response IPC.
//!
//! Each request is identified by the document that made it and its
//! correlation id. Correlation ids are local to a renderer process.
//!
//! Only the document that made a request may cancel it. The request ends
//! with that document.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::ipc::document::DocumentKey;

/// Pending async entry that can be cancelled via its abort flag.
#[derive(Clone)]
pub struct PendingEntry {
    pub aborted: Arc<AtomicBool>, // Coordination mechanism between cancel and resolve
}

/// Identifies one pending request.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct PendingKey {
    document: DocumentKey,
    id: i32,
}

impl PendingKey {
    pub(crate) fn new(document: DocumentKey, id: i32) -> Self {
        Self { document, id }
    }
}

/// Thread-safe handle to the pending map. Clones share one map.
#[derive(Clone, Default)]
pub struct PendingMap {
    /// Entries by document, then by correlation id.
    inner: Arc<Mutex<HashMap<DocumentKey, HashMap<i32, PendingEntry>>>>,
}

impl PendingMap {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&self, key: PendingKey, entry: PendingEntry) {
        self.inner
            .lock()
            .unwrap()
            .entry(key.document)
            .or_default()
            .insert(key.id, entry);
    }

    pub fn remove(&self, key: &PendingKey) -> Option<PendingEntry> {
        let mut documents = self.inner.lock().unwrap();
        let entries = documents.get_mut(&key.document)?;
        let entry = entries.remove(&key.id);
        if entries.is_empty() {
            documents.remove(&key.document);
        }
        entry
    }

    /// Aborts and removes the entry; false when there was none.
    pub fn cancel(&self, key: &PendingKey) -> bool {
        match self.remove(key) {
            Some(entry) => {
                entry.aborted.store(true, Ordering::SeqCst);
                true
            }
            None => false,
        }
    }

    /// Aborts and removes every entry `document` made; returns how many.
    pub(crate) fn cancel_document(&self, document: DocumentKey) -> usize {
        let entries = self
            .inner
            .lock()
            .unwrap()
            .remove(&document)
            .unwrap_or_default();
        for entry in entries.values() {
            entry.aborted.store(true, Ordering::SeqCst);
        }
        entries.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acl::Origin;
    use crate::ipc::document::Document;
    use crate::ipc::responder::Responder;
    use std::sync::atomic::AtomicUsize;

    fn document() -> DocumentKey {
        Document::for_test(Origin::parse("app://app").unwrap()).key()
    }

    fn entry() -> PendingEntry {
        PendingEntry {
            aborted: Arc::new(AtomicBool::new(false)),
        }
    }

    #[test]
    fn entries_are_found_only_by_their_exact_key() {
        let (a, b) = (document(), document());
        let map = PendingMap::new();
        map.insert(PendingKey::new(a, 10), entry());
        assert!(
            map.remove(&PendingKey::new(b, 10)).is_none(),
            "other document"
        );
        assert!(map.remove(&PendingKey::new(a, 99)).is_none(), "other id");
        assert!(map.remove(&PendingKey::new(a, 10)).is_some());
        assert!(
            map.remove(&PendingKey::new(a, 10)).is_none(),
            "removed once"
        );
    }

    #[test]
    fn documents_with_the_same_correlation_id_do_not_collide() {
        let (main, iframe) = (document(), document());
        let map = PendingMap::new();
        let first = entry();
        let first_flag = first.aborted.clone();
        map.insert(PendingKey::new(main, 1), first);
        map.insert(PendingKey::new(iframe, 1), entry());

        assert!(map.cancel(&PendingKey::new(iframe, 1)));
        assert!(
            !first_flag.load(Ordering::SeqCst),
            "the iframe cannot cancel the main frame's request"
        );
        assert!(map.remove(&PendingKey::new(main, 1)).is_some());
    }

    #[test]
    fn cancel_sets_the_flag_and_removes_the_entry_once() {
        let a = document();
        let map = PendingMap::new();
        let e = entry();
        let flag = e.aborted.clone();
        map.insert(PendingKey::new(a, 0), e);
        assert!(map.cancel(&PendingKey::new(a, 0)));
        assert!(flag.load(Ordering::SeqCst));
        assert!(!map.cancel(&PendingKey::new(a, 0)));
        assert!(map.remove(&PendingKey::new(a, 0)).is_none());
    }

    #[test]
    fn cancel_document_touches_only_that_document() {
        let (gone, other) = (document(), document());
        let map = PendingMap::new();
        let (first, second, third) = (entry(), entry(), entry());
        let flags = [
            first.aborted.clone(),
            second.aborted.clone(),
            third.aborted.clone(),
        ];
        map.insert(PendingKey::new(gone, 1), first);
        map.insert(PendingKey::new(gone, 2), second);
        map.insert(PendingKey::new(other, 1), third);

        assert_eq!(map.cancel_document(gone), 2);
        assert_eq!(map.cancel_document(gone), 0);
        assert!(flags[0].load(Ordering::SeqCst) && flags[1].load(Ordering::SeqCst));
        assert!(!flags[2].load(Ordering::SeqCst));
        assert!(map.remove(&PendingKey::new(other, 1)).is_some());
    }

    #[test]
    fn clones_share_state() {
        let a = PendingMap::new();
        let b = a.clone();
        let key = PendingKey::new(document(), 10);
        a.insert(key, entry());
        assert!(b.remove(&key).is_some());
    }

    #[test]
    fn concurrent_insert_cancel_and_cancel_document_are_safe() {
        let map = PendingMap::new();
        let documents = [document(), document()];
        let handles: Vec<_> = (0..8)
            .map(|t| {
                let map = map.clone();
                let document = documents[t % 2];
                std::thread::spawn(move || {
                    for i in 0..100 {
                        map.insert(PendingKey::new(document, i), entry());
                        map.cancel(&PendingKey::new(document, i));
                        if i % 25 == 0 {
                            map.cancel_document(document);
                        }
                    }
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }
        for document in documents {
            map.cancel_document(document);
        }
        assert!(map.remove(&PendingKey::new(documents[0], 0)).is_none());
    }

    #[test]
    fn cancellation_suppresses_late_responses() {
        let a = document();
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
            map.insert(PendingKey::new(a, id), e);
        }
        assert!(map.cancel(&PendingKey::new(a, 0)));
        assert_eq!(map.cancel_document(a), 9);
        for responder in responders {
            responder.resolve(Ok(()));
        }
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }
}
