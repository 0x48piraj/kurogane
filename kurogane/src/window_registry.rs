use std::collections::HashMap;
use cef::{Window, ImplWindow};
use crate::browser_registry::BrowserId;
use crate::debug;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WindowId(u32);

impl WindowId {
    pub fn as_u32(&self) -> u32 {
        self.0
    }
}

#[derive(Debug, Clone)]
pub struct WindowMetadata {
    #[allow(dead_code)]
    pub id: WindowId,
    #[allow(dead_code)]
    pub created_at: std::time::Instant,
}

pub(crate) struct WindowState {
    pub window: Window,
    pub browser_id: Option<BrowserId>,
    #[allow(dead_code)]
    pub metadata: WindowMetadata,
}

pub(crate) struct WindowRegistry {
    windows: HashMap<WindowId, WindowState>,
    lookup: HashMap<BrowserId, WindowId>,
    next_id: u32,
}

impl WindowRegistry {
    pub fn new() -> Self {
        Self {
            windows: HashMap::new(),
            lookup: HashMap::new(),
            next_id: 1,
        }
    }

    pub fn allocate_id(&mut self) -> WindowId {
        let id = WindowId(self.next_id);
        self.next_id += 1;
        id
    }

    pub fn insert(
        &mut self,
        id: WindowId,
        window: Window,
        browser_id: Option<BrowserId>,
    ) {
        let state = WindowState {
            window,
            browser_id,
            metadata: WindowMetadata {
                id,
                created_at: std::time::Instant::now(),
            },
        };

        debug!(
            "[WindowRegistry] registered window {} (browser={:?})",
            id.as_u32(),
            browser_id
        );

        if let Some(bid) = browser_id {
            self.lookup.insert(bid, id);
        }

        self.windows.insert(id, state);
    }

    pub fn unregister(&mut self, id: WindowId) -> bool {
        if let Some(state) = self.windows.remove(&id) {
            if let Some(bid) = state.browser_id {
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

    #[allow(dead_code)]
    pub fn get(&self, id: WindowId) -> Option<&WindowState> {
        self.windows.get(&id)
    }

    #[allow(dead_code)]
    pub fn get_mut(&mut self, id: WindowId) -> Option<&mut WindowState> {
        self.windows.get_mut(&id)
    }

    pub fn close_all_windows(&self) {
        let windows: Vec<Window> = self.windows.values().map(|s| s.window.clone()).collect();
        for w in windows {
            w.close();
        }
    }

    pub fn window_id_for_browser(&self, browser_id: BrowserId) -> Option<WindowId> {
        self.lookup.get(&browser_id).copied()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&WindowId, &WindowState)> {
        self.windows.iter()
    }
}
