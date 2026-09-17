//! Event IPC subsystem.
//!
//! Provides publish/subscribe event delivery between the browser and renderer processes.
//! Renderers subscribe to named events and the browser emits events to subscribed frames.

use std::collections::HashMap;
use std::sync::Mutex;

use cef::{Frame, ImplFrame};

use crate::acl::Origin;
use crate::browser_registry::BrowserId;
use crate::ipc::{FrameId, IpcContext};

/// Browser-side event subscription.
pub struct EventSubscription {
    pub id: u32,
    pub frame: Frame,
    pub browser_id: BrowserId,

    /// Frame that created the subscription.
    pub frame_id: FrameId,

    /// Origin authorized for the subscription.
    pub origin: Origin,

    /// URL origin of the document that created the subscription. Events are
    /// delivered only while the frame still shows this origin.
    pub url_origin: Origin,
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

    /// Removes the subscriptions of `frame`, which is starting a new
    /// document.
    ///
    /// Returns the number of subscriptions removed.
    pub fn clear_frame(&self, frame: &FrameId) -> usize {
        let mut subs = self.subscriptions.lock().unwrap();
        let mut total = 0;
        subs.retain(|_, v| {
            let before = v.len();
            v.retain(|s| s.frame_id != *frame);
            total += before - v.len();
            !v.is_empty()
        });
        total
    }

    /// Removes all subscriptions whose frame is no longer valid.
    ///
    /// Returns the number of subscriptions removed.
    pub fn clear_invalid_frames(&self) -> usize {
        let mut subs = self.subscriptions.lock().unwrap();
        let mut total = 0;
        subs.retain(|_, v| {
            let before = v.len();
            v.retain(|s| s.frame.is_valid() != 0);
            let removed = before - v.len();
            if removed > 0 {
                crate::debug!(
                    "[EventSubsystem] removed {} subscription(s) with invalid frame",
                    removed
                );
            }
            total += removed;
            !v.is_empty()
        });
        total
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

    /// Removes subscription `id` of `event_name` if the frame and origin of
    /// `ctx` made it, returning true if found.
    pub fn remove_subscription(&self, event_name: &str, id: u32, ctx: &IpcContext) -> bool {
        let mut subs = self.subscriptions.lock().unwrap();
        let Some(entries) = subs.get_mut(event_name) else {
            return false;
        };
        let before = entries.len();
        entries.retain(|s| {
            let owner = Some(s.browser_id) == ctx.browser_id
                && s.frame_id == ctx.frame
                && s.origin == ctx.origin;
            !(owner && s.id == id)
        });
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
