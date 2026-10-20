//! Documents the browser exchanges messages with.
//!
//! A document is what one frame shows from its commit until the frame goes
//! or another document commits in it. Its origin is read once, as it starts.
//! What a document opens holds it and stops once it ends.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use tanso::{CefString, Frame, ImplFrame};

use crate::acl::Origin;
use crate::browser_registry::BrowserId;
use crate::ipc::FrameId;

/// Identity of a document, unique within the process.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct DocumentKey(u64);

/// One document of one frame.
#[derive(Debug)]
pub struct Document {
    key: DocumentKey,
    browser: BrowserId,
    frame: FrameId,
    /// Origin of the document's URL as it started.
    origin: Origin,
    live: AtomicBool,
}

impl Document {
    fn start(browser: BrowserId, frame: FrameId, origin: Origin) -> Arc<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Arc::new(Self {
            key: DocumentKey(NEXT.fetch_add(1, Ordering::Relaxed)),
            browser,
            frame,
            origin,
            live: AtomicBool::new(true),
        })
    }

    #[cfg(test)]
    pub(crate) fn for_test(origin: Origin) -> Arc<Self> {
        Self::start(BrowserId::new(1), FrameId::new("test-frame"), origin)
    }

    #[cfg(test)]
    pub(crate) fn ended_for_test(origin: Origin) -> Arc<Self> {
        let document = Self::for_test(origin);
        document.end();
        document
    }

    pub(crate) fn key(&self) -> DocumentKey {
        self.key
    }

    pub(crate) fn browser(&self) -> BrowserId {
        self.browser
    }

    pub(crate) fn frame(&self) -> &FrameId {
        &self.frame
    }

    pub(crate) fn origin(&self) -> &Origin {
        &self.origin
    }

    /// Returns whether the document still shows in its frame.
    pub(crate) fn is_live(&self) -> bool {
        self.live.load(Ordering::SeqCst)
    }

    fn end(&self) {
        self.live.store(false, Ordering::SeqCst);
    }
}

/// Current document of every frame the browser has heard from.
#[derive(Default)]
pub(crate) struct Documents {
    by_frame: Mutex<HashMap<FrameId, Current>>,
}

struct Current {
    document: Arc<Document>,
    /// Whether the document's commit was seen, rather than only its messages.
    committed: bool,
}

impl Documents {
    /// Returns the document frame `id` shows, started from `frame`'s URL
    /// when the frame is new. A frame that has gone shows none.
    pub(crate) fn current(
        &self,
        frame: &Frame,
        id: &FrameId,
        browser: BrowserId,
    ) -> Option<Arc<Document>> {
        let mut by_frame = self.lock();
        if let Some(current) = by_frame.get(id) {
            return Some(current.document.clone());
        }
        if frame.is_valid() == 0 {
            return None;
        }
        let document = Document::start(browser, id.clone(), origin_of(frame));
        by_frame.insert(
            id.clone(),
            Current {
                document: document.clone(),
                committed: false,
            },
        );
        Some(document)
    }

    /// Starts the document `frame` commits. Returns the document it ends.
    ///
    /// A document whose messages came before its commit stays since it is
    /// the one committing.
    pub(crate) fn commit(&self, frame: &Frame, browser: BrowserId) -> Option<Arc<Document>> {
        let id = FrameId::of(frame);
        let origin = origin_of(frame);
        let mut by_frame = self.lock();
        if let Some(current) = by_frame.get_mut(&id)
            && !current.committed
            && current.document.origin == origin
        {
            current.committed = true;
            return None;
        }
        let document = Document::start(browser, id.clone(), origin);
        let ended = by_frame.insert(
            id,
            Current {
                document,
                committed: true,
            },
        )?;
        ended.document.end();
        Some(ended.document)
    }

    /// Ends the document of `frame`, which has gone. Returns it.
    pub(crate) fn frame_gone(&self, frame: &Frame) -> Option<Arc<Document>> {
        let ended = self.lock().remove(&FrameId::of(frame))?;
        ended.document.end();
        Some(ended.document)
    }

    /// Ends every document of `browser`, which has closed. Returns them.
    pub(crate) fn browser_closed(&self, browser: BrowserId) -> Vec<Arc<Document>> {
        let mut ended = Vec::new();
        self.lock().retain(|_, current| {
            if current.document.browser != browser {
                return true;
            }
            current.document.end();
            ended.push(current.document.clone());
            false
        });
        ended
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<FrameId, Current>> {
        self.by_frame.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Returns the origin of the URL `frame` shows.
fn origin_of(frame: &Frame) -> Origin {
    Origin::from_url(&CefString::from(&frame.url()).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn documents_are_told_apart_by_key() {
        let app = Origin::parse("app://app").unwrap();
        let (a, b) = (Document::for_test(app.clone()), Document::for_test(app));
        assert_ne!(a.key(), b.key());
    }

    #[test]
    fn a_document_is_live_until_it_ends() {
        let document = Document::for_test(Origin::OPAQUE);
        assert!(document.is_live());
        document.end();
        assert!(!document.is_live());
    }
}
