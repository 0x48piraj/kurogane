//! The browsers and windows the runtime tracks.

use crate::ShutdownSignal;
use crate::browser_registry::BrowserRegistry;
use crate::window_registry::WindowRegistry;

/// The open browsers and windows, and which window shows which browser.
///
/// Plain data: nothing in it makes a CEF call that can call back into
/// Kurogane, so all of it is safe under the lock `AppHandle::registry` takes.
pub(crate) struct Registry {
    pub(crate) browsers: BrowserRegistry,
    pub(crate) windows: WindowRegistry,
}

impl Registry {
    pub(crate) fn new(shutdown_signal: ShutdownSignal) -> Self {
        Self {
            browsers: BrowserRegistry::new(shutdown_signal),
            windows: WindowRegistry::new(),
        }
    }
}
