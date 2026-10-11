use std::collections::HashMap;
use tanso::{Rect, Window};
use crate::browser_registry::BrowserId;
use crate::window_closing::Closing;
use crate::window_options::WindowState;
use tracing::debug;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WindowId(u32);

impl WindowId {
    pub fn as_u32(&self) -> u32 {
        self.0
    }
}

#[derive(Debug, Clone)]
pub struct WindowMetadata {
    pub id: WindowId,
    pub created_at: std::time::Instant,
}

pub(crate) struct WindowEntry {
    pub window: Window,
    pub browser_id: Option<BrowserId>,
    pub metadata: WindowMetadata,
    /// Whether the window takes its page's title (crate::window)
    pub follows_title: bool,
    pub kind: WindowKind,
    /// Where the window is when neither maximized, minimized nor fullscreen:
    /// what it restores to (crate::window)
    pub restored: Option<Rect>,
    /// The state it showed in when last not minimized: what it comes back
    /// as once restored (crate::window_closing)
    pub shown: WindowState,
}

/// What opened a window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WindowKind {
    /// The application's: the start window, create_window's, or one a page
    /// opened as the application's. The application hears of its close
    /// (crate::window_closing)
    Application,
    /// A page's popup, or DevTools
    Popup,
}

pub(crate) struct WindowRegistry {
    windows: HashMap<WindowId, WindowEntry>,
    lookup: HashMap<BrowserId, WindowId>,
    /// The names the application gave its windows (WindowOptions::name),
    /// each held by its window from its ID on until its close is reported
    names: HashMap<String, WindowId>,
    next_id: u32,
}

impl WindowRegistry {
    pub fn new() -> Self {
        Self {
            windows: HashMap::new(),
            lookup: HashMap::new(),
            names: HashMap::new(),
            next_id: 1,
        }
    }

    pub fn allocate_id(&mut self) -> WindowId {
        let id = WindowId(self.next_id);
        self.next_id += 1;
        id
    }

    /// An ID for a new window named `name`, which holds the name from now
    /// on; or, when an open window holds it already, that window.
    pub fn allocate_named(&mut self, name: &str) -> Result<WindowId, WindowId> {
        if let Some(&holder) = self.names.get(name) {
            return Err(holder);
        }
        let id = self.allocate_id();
        self.names.insert(name.to_owned(), id);
        Ok(id)
    }

    /// Lets go of the name window `id` holds, and returns it.
    pub fn release_name(&mut self, id: WindowId) -> Option<String> {
        let name = self
            .names
            .iter()
            .find_map(|(name, holder)| (*holder == id).then(|| name.clone()))?;
        self.names.remove(&name);
        Some(name)
    }

    /// The window that holds `name`.
    pub fn window_named(&self, name: &str) -> Option<WindowId> {
        self.names.get(name).copied()
    }

    pub fn insert(
        &mut self,
        id: WindowId,
        window: Window,
        browser_id: Option<BrowserId>,
        follows_title: bool,
        kind: WindowKind,
        shown: WindowState,
    ) {
        let entry = WindowEntry {
            window,
            browser_id,
            metadata: WindowMetadata {
                id,
                created_at: std::time::Instant::now(),
            },
            follows_title,
            kind,
            restored: None,
            shown,
        };

        debug!(
            "[WindowRegistry] registered window {} (browser={:?})",
            id.as_u32(),
            browser_id
        );

        if let Some(bid) = browser_id {
            self.lookup.insert(bid, id);
        }

        self.windows.insert(id, entry);
    }

    pub fn unregister(&mut self, id: WindowId) -> bool {
        // Let go already when its browser closed, unless it never had one
        self.release_name(id);
        if let Some(entry) = self.windows.remove(&id) {
            if let Some(bid) = entry.browser_id {
                self.lookup.remove(&bid);
            }
            debug!("[WindowRegistry] unregistered window {}", id.0);
            true
        } else {
            false
        }
    }

    pub fn count(&self) -> usize {
        self.windows.len()
    }

    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.windows.is_empty()
    }

    pub fn get(&self, id: WindowId) -> Option<&WindowEntry> {
        self.windows.get(&id)
    }

    #[allow(dead_code)]
    pub fn get_mut(&mut self, id: WindowId) -> Option<&mut WindowEntry> {
        self.windows.get_mut(&id)
    }

    /// Records that window `id` restores to `bounds` (crate::window).
    pub fn set_restored(&mut self, id: WindowId, bounds: Rect) {
        if let Some(entry) = self.windows.get_mut(&id) {
            entry.restored = Some(bounds);
        }
    }

    /// Records that window `id`, not minimized, shows in `state` at
    /// `bounds` (crate::window): where it restores to when it shows normally;
    /// a maximized or fullscreen window's bounds are its display's.
    pub fn shown(&mut self, id: WindowId, state: WindowState, bounds: &Rect) {
        if let Some(entry) = self.windows.get_mut(&id) {
            entry.shown = state;
            if state == WindowState::Normal {
                entry.restored = Some(bounds.clone());
            }
        }
    }

    /// The application window `id`, whose browser has closed, as its close
    /// is reported (crate::window_closing): it lets go of its name. None for
    /// a popup's window.
    pub fn closing(&mut self, id: WindowId) -> Option<Closing> {
        let entry = self.windows.get(&id)?;
        let (window, restored, shown) = match entry.kind {
            WindowKind::Application => (entry.window.clone(), entry.restored.clone(), entry.shown),
            WindowKind::Popup => return None,
        };
        Some(Closing {
            id,
            name: self.release_name(id),
            window,
            restored,
            shown,
        })
    }

    /// Every registered window.
    pub fn all(&self) -> Vec<Window> {
        self.windows.values().map(|s| s.window.clone()).collect()
    }

    /// Returns windows whose browser is still being created or has already gone.
    pub fn unlinked(&self) -> Vec<Window> {
        self.windows
            .values()
            .filter(|s| s.browser_id.is_none())
            .map(|s| s.window.clone())
            .collect()
    }

    pub fn window_id_for_browser(&self, browser_id: BrowserId) -> Option<WindowId> {
        self.lookup.get(&browser_id).copied()
    }

    /// Records that `browser_id` is the browser shown in window `id`.
    ///
    /// Returns false when no such window is registered.
    pub fn link(&mut self, id: WindowId, browser_id: BrowserId) -> bool {
        let Some(entry) = self.windows.get_mut(&id) else {
            return false;
        };
        entry.browser_id = Some(browser_id);
        self.lookup.insert(browser_id, id);
        true
    }

    /// Forgets that a window shows `browser_id`, which has closed. The
    /// window stays registered until CEF destroys it.
    pub fn unlink_browser(&mut self, browser_id: BrowserId) {
        if let Some(id) = self.lookup.remove(&browser_id)
            && let Some(entry) = self.windows.get_mut(&id)
            && entry.browser_id == Some(browser_id)
        {
            entry.browser_id = None;
            debug!(
                "[WindowRegistry] unlinked browser {} from window {}",
                browser_id.as_u32(),
                id.as_u32()
            );
        }
    }

    pub fn browser_for_window(&self, id: WindowId) -> Option<BrowserId> {
        self.windows.get(&id).and_then(|s| s.browser_id)
    }

    #[allow(dead_code)]
    pub fn metadata(&self, id: WindowId) -> Option<WindowMetadata> {
        self.windows.get(&id).map(|s| s.metadata.clone())
    }

    pub fn iter(&self) -> impl Iterator<Item = (&WindowId, &WindowEntry)> {
        self.windows.iter()
    }
}
