//! The browsers and windows the runtime tracks.

use cef::Browser;

use crate::browser_registry::{BrowserId, BrowserRegistry, BrowserType};
use crate::window_registry::WindowRegistry;

/// The open browsers and windows, and which window shows which browser.
///
/// Plain data: nothing in it makes a CEF call that can call back into
/// Kurogane, so all of it is safe under the lock `AppHandle::registry` takes.
pub(crate) struct Registry {
    pub(crate) browsers: BrowserRegistry,
    pub(crate) windows: WindowRegistry,
}

/// A browser CEF has closed, and what its close leaves to do once the
/// registry's guard is gone.
pub(crate) struct Closed {
    pub(crate) id: BrowserId,
    /// Browsers Chromium opened on its own, which close with the
    /// application's last browser
    pub(crate) stragglers: Vec<Browser>,
    /// No browser is left
    pub(crate) last: bool,
}

impl Registry {
    pub(crate) fn new() -> Self {
        Self {
            browsers: BrowserRegistry::new(),
            windows: WindowRegistry::new(),
        }
    }

    /// Forgets `browser`, which CEF has closed (OnBeforeClose), and its
    /// window's link to it in the same update, so no reader sees a window
    /// that names a closed browser. None for a browser never registered.
    pub(crate) fn browser_closed(&mut self, browser: &Browser) -> Option<Closed> {
        let id = self.browsers.find_id_by_browser(browser)?;
        let was_app_browser = self
            .browsers
            .get(id)
            .is_some_and(|state| state.metadata.browser_type != BrowserType::ChromeUi);

        self.browsers.unregister(id);
        self.windows.unlink_browser(id);

        // Windows Chromium opened on its own close with the application's
        // last one rather than keep the process running
        let stragglers = if was_app_browser && !self.browsers.has_app_browsers() {
            self.browsers.chrome_ui_browsers()
        } else {
            Vec::new()
        };

        Some(Closed {
            id,
            stragglers,
            last: self.browsers.is_empty(),
        })
    }
}
