//! What a page changes about itself that the application may follow: its
//! title ([`App::on_title_change`](crate::App::on_title_change)) and
//! fullscreen ([`App::on_fullscreen_change`](crate::App::on_fullscreen_change)).
//!
//! CEF tells its display handler of both. Kurogane titles a window after
//! its page itself (crate::window) and CEF's window enters and leaves
//! fullscreen with the page; the hooks only hear of it. DevTools and
//! Chromium's own browsers are not reported.

use std::panic::{AssertUnwindSafe, catch_unwind};

use tracing::error;

use crate::browser_registry::BrowserId;
use crate::runtime::AppHandle;
use crate::window_registry::WindowId;

/// A page's new title, passed to
/// [`App::on_title_change`](crate::App::on_title_change).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TitleChange {
    title: String,
    browser: Option<BrowserId>,
    window: Option<WindowId>,
}

impl TitleChange {
    pub(crate) fn new(title: String, browser: Option<BrowserId>, window: Option<WindowId>) -> Self {
        Self {
            title,
            browser,
            window,
        }
    }

    /// The title the page gave itself: its `<title>`, or what a script set
    /// `document.title` to.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// The browser of the page.
    pub fn browser(&self) -> Option<BrowserId> {
        self.browser
    }

    /// The window the page is in; none for a browser embedded in the
    /// application's own window.
    pub fn window(&self) -> Option<WindowId> {
        self.window
    }
}

/// A page entering or leaving fullscreen, passed to
/// [`App::on_fullscreen_change`](crate::App::on_fullscreen_change).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FullscreenChange {
    fullscreen: bool,
    browser: Option<BrowserId>,
    window: Option<WindowId>,
}

impl FullscreenChange {
    pub(crate) fn new(
        fullscreen: bool,
        browser: Option<BrowserId>,
        window: Option<WindowId>,
    ) -> Self {
        Self {
            fullscreen,
            browser,
            window,
        }
    }

    /// Whether the page is fullscreen now: it asked with
    /// `requestFullscreen()`, or left with `exitFullscreen()` or Escape.
    pub fn is_fullscreen(&self) -> bool {
        self.fullscreen
    }

    /// The browser of the page.
    pub fn browser(&self) -> Option<BrowserId> {
        self.browser
    }

    /// The window the page is in; none for a browser embedded in the
    /// application's own window.
    pub fn window(&self) -> Option<WindowId> {
        self.window
    }
}

/// Tells the application's hook of `change`. UI thread, no lock held. A
/// hook that panics is logged.
pub(crate) fn report_title(app: &AppHandle, change: &TitleChange) {
    let hooks = app.hooks();
    let Some(hook) = hooks
        .as_deref()
        .and_then(|hooks| hooks.title_change.as_ref())
    else {
        return;
    };
    if catch_unwind(AssertUnwindSafe(|| hook(change, app))).is_err() {
        error!("on_title_change panicked");
    }
}

/// Tells the application's hook of `change`. UI thread, no lock held. A
/// hook that panics is logged.
pub(crate) fn report_fullscreen(app: &AppHandle, change: &FullscreenChange) {
    let hooks = app.hooks();
    let Some(hook) = hooks
        .as_deref()
        .and_then(|hooks| hooks.fullscreen_change.as_ref())
    else {
        return;
    };
    if catch_unwind(AssertUnwindSafe(|| hook(change, app))).is_err() {
        error!("on_fullscreen_change panicked");
    }
}
