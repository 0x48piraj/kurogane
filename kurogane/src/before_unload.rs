//! A page's `beforeunload` asking to stay, and the application answering it
//! ([`App::on_before_unload`](crate::App::on_before_unload)).
//!
//! A page that cancels `beforeunload` makes Chromium ask the user whether to
//! leave, in a dialog of its own. CEF lets the client answer instead, through
//! its JS dialog handler: with the hook, the application answers, and
//! Chromium's dialog never shows. Without it nothing changes.

use std::panic::{AssertUnwindSafe, catch_unwind};

use tracing::error;

use crate::browser_registry::BrowserId;
use crate::runtime::AppHandle;

/// A page asking to stay as its browser unloads, passed to
/// [`App::on_before_unload`](crate::App::on_before_unload).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BeforeUnload {
    browser: BrowserId,
    reload: bool,
}

impl BeforeUnload {
    pub(crate) fn new(browser: BrowserId, reload: bool) -> Self {
        Self { browser, reload }
    }

    /// The browser whose page asked.
    pub fn browser(&self) -> BrowserId {
        self.browser
    }

    /// Whether the page is being reloaded rather than closed or navigated
    /// away from.
    pub fn is_reload(&self) -> bool {
        self.reload
    }
}

/// What the application answers a page that asked to stay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnloadDecision {
    /// Chromium asks the user in its own dialog, as without the hook.
    Ask,
    /// The page unloads: the browser closes, reloads or navigates.
    Leave,
    /// The page stays, and so does its browser and window.
    Stay,
}

/// The application's answer for `request`, or None with no hook. A hook that
/// panics is logged and answered as [`UnloadDecision::Ask`].
pub(crate) fn decide(app: &AppHandle, request: &BeforeUnload) -> Option<UnloadDecision> {
    let hooks = app.hooks()?;
    let hook = hooks.before_unload.as_ref()?;
    match catch_unwind(AssertUnwindSafe(|| hook(request, app))) {
        Ok(decision) => Some(decision),
        Err(_) => {
            error!("on_before_unload panicked; Chromium asks the user");
            Some(UnloadDecision::Ask)
        }
    }
}
