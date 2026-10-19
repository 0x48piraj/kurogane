//! The browsers and windows the runtime tracks.

use tanso::Browser;

use crate::browser_registry::{BrowserEntry, BrowserId, BrowserRegistry, BrowserType};
use crate::window_closing::Closing;
use crate::window_registry::WindowRegistry;

/// Open browsers, open windows and which window shows which browser.
///
/// Nothing in it makes a CEF call that can call back into Kurogane. All of
/// it is safe under the lock `AppHandle::registry` takes. Releasing what it
/// holds of CEF's can call back; a closed browser's entry leaves with its
/// [`Closed`].
pub(crate) struct Registry {
    pub(crate) browsers: BrowserRegistry,
    pub(crate) windows: WindowRegistry,
}

/// A browser CEF has closed with what its close leaves to do once the
/// registry's guard is gone.
pub(crate) struct Closed {
    pub(crate) id: BrowserId,
    /// Browsers Chromium opened on its own, which close with the
    /// application's last browser
    pub(crate) stragglers: Vec<Browser>,
    /// No browser is left
    pub(crate) last: bool,
    /// The browser's entry, to release outside the lock. CEF calls back into
    /// Kurogane as what it holds goes (a download waiting for its Save As
    /// cancels, a waiting permission request is answered). Its waiting
    /// permissions are to deny and its file dialogs to cancel
    pub(crate) entry: BrowserEntry,
    /// The application window the browser showed, whose close the
    /// application hears of (crate::window_closing)
    pub(crate) closing: Option<Closing>,
}

impl Registry {
    pub(crate) fn new() -> Self {
        Self {
            browsers: BrowserRegistry::new(),
            windows: WindowRegistry::new(),
        }
    }

    /// Forgets `browser`, which CEF has closed (OnBeforeClose), together with
    /// its window's link to it in one update. No reader sees a window that
    /// names a closed browser. An application window lets go of its name in
    /// it too. None for a browser never registered.
    pub(crate) fn browser_closed(&mut self, browser: &Browser) -> Option<Closed> {
        let id = self.browsers.find_id_by_browser(browser)?;
        let window = self.windows.window_id_for_browser(id);
        let entry = self.browsers.unregister(id)?;
        self.windows.unlink_browser(id);
        let closing = window.and_then(|window| self.windows.closing(window));

        // Windows Chromium opened on its own close with the application's
        // last one rather than keep the process running
        let stragglers = if entry.metadata.browser_type != BrowserType::ChromeUi
            && !self.browsers.has_app_browsers()
        {
            self.browsers.chrome_ui_browsers()
        } else {
            Vec::new()
        };

        Some(Closed {
            id,
            stragglers,
            last: self.browsers.is_empty(),
            entry,
            closing,
        })
    }
}
