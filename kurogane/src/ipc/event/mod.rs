//! Event IPC subsystem.
//!
//! Provides publish/subscribe event delivery between the browser and renderer processes.
//! Renderers subscribe to named events and the browser emits events to subscribed frames.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tanso::Frame;

use crate::ipc::IpcContext;
use crate::ipc::document::{Document, DocumentKey};

/// Browser-side event subscription.
pub struct EventSubscription {
    pub id: u32,
    pub frame: Frame,

    /// Document that made the subscription. Events reach it while it lives.
    pub document: Arc<Document>,
}

pub mod browser;
pub mod renderer;

/// Browser-side event dispatcher.
pub struct EventSubsystem {
    pub subscriptions: Mutex<HashMap<String, Vec<EventSubscription>>>,
}

impl EventSubsystem {
    pub fn new() -> Self {
        Self {
            subscriptions: Mutex::new(HashMap::new()),
        }
    }

    /// Removes the subscriptions of `document`, which has ended.
    ///
    /// Returns the number of subscriptions removed.
    pub(crate) fn remove_document(&self, document: DocumentKey) -> usize {
        let mut subs = self.subscriptions.lock().unwrap();
        let mut total = 0;
        subs.retain(|_, v| {
            let before = v.len();
            v.retain(|s| s.document.key() != document);
            total += before - v.len();
            !v.is_empty()
        });
        total
    }

    /// Removes subscription `id` of `event_name` if the document of `ctx`
    /// made it, returning true if found.
    pub fn remove_subscription(&self, event_name: &str, id: u32, ctx: &IpcContext) -> bool {
        let mut subs = self.subscriptions.lock().unwrap();
        let Some(entries) = subs.get_mut(event_name) else {
            return false;
        };
        let before = entries.len();
        entries.retain(|s| !(s.document.key() == ctx.document.key() && s.id == id));
        let removed = before - entries.len();
        if entries.is_empty() {
            subs.remove(event_name);
        }
        removed > 0
    }
}

impl Default for EventSubsystem {
    fn default() -> Self {
        Self::new()
    }
}
