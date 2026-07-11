//! Event IPC subsystem.
//!
//! Provides publish/subscribe event delivery between the browser and renderer processes.
//! Renderers subscribe to named events and the browser emits events to subscribed frames.

use std::collections::HashMap;
use std::sync::Mutex;

use cef::Frame;

use crate::browser_registry::BrowserId;

/// Browser-side event subscription.
pub struct EventSubscription {
    pub id: u32,
    pub frame: Frame,
    pub browser_id: BrowserId,
    pub persistent: bool,
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

    /// Removes all subscriptions associated with a browser.
    ///
    /// Returns the number of subscriptions removed.
    pub fn clear_for_browser(&self, browser_id: BrowserId) -> usize {
        let mut subs = self.subscriptions.lock().unwrap();
        let mut total = 0;
        subs.retain(|_, v| {
            let before = v.len();
            v.retain(|s| s.browser_id != browser_id);
            let removed = before - v.len();
            total += removed;
            !v.is_empty()
        });
        total
    }

    /// Removes a single subscription by id, returning true if found.
    pub fn remove_subscription(&self, event_name: &str, browser_id: BrowserId, id: u32) -> bool {
        let mut subs = self.subscriptions.lock().unwrap();
        let Some(entries) = subs.get_mut(event_name) else {
            return false;
        };
        let before = entries.len();
        entries.retain(|s| !(s.browser_id == browser_id && s.id == id));
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
