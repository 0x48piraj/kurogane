//! The browsers and windows the runtime tracks.

use cef::{Browser, FileDialogCallback};

use crate::browser_registry::{BrowserId, BrowserRegistry, BrowserType};
use crate::permissions::Pending;
use crate::window_closing::Closing;
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
    /// The browser's permission requests still waiting for an answer, to
    /// deny: CEF answers a page's request for a device as its callback
    /// goes, which must not happen under the lock
    pub(crate) waiting_permissions: Vec<Pending>,
    /// The browser's file dialogs still waiting for an answer, to cancel
    /// outside the lock
    pub(crate) waiting_file_dialogs: Vec<FileDialogCallback>,
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

    /// Forgets `browser`, which CEF has closed (OnBeforeClose), and its
    /// window's link to it in the same update, so no reader sees a window
    /// that names a closed browser; an application window lets go of its
    /// name in it too. None for a browser never registered.
    pub(crate) fn browser_closed(&mut self, browser: &Browser) -> Option<Closed> {
        let id = self.browsers.find_id_by_browser(browser)?;
        let window = self.windows.window_id_for_browser(id);
        let was_app_browser = self
            .browsers
            .get(id)
            .is_some_and(|state| state.metadata.browser_type != BrowserType::ChromeUi);
        let waiting_permissions = self
            .browsers
            .get_mut(id)
            .map(|state| state.permissions.take_all())
            .unwrap_or_default();
        let waiting_file_dialogs = self
            .browsers
            .get_mut(id)
            .map(|state| state.file_dialogs.take_all())
            .unwrap_or_default();

        self.browsers.unregister(id);
        self.windows.unlink_browser(id);
        let closing = window.and_then(|window| self.windows.closing(window));

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
            waiting_permissions,
            waiting_file_dialogs,
            closing,
        })
    }
}
