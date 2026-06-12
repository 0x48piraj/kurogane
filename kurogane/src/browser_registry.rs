use std::collections::HashMap;
use cef::{Browser, ImplBrowser, RequestContext};
use crate::ShutdownSignal;
use crate::debug;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BrowserId(u32);

impl BrowserId {
    pub fn as_u32(&self) -> u32 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowserType {
    Main,
    Popup,
    DevTools,
    #[allow(dead_code)]
    Osr,
}

#[derive(Debug, Clone)]
pub struct BrowserMetadata {
    pub id: BrowserId,
    pub browser_type: BrowserType,
    pub parent_id: Option<BrowserId>,
    pub opener_id: Option<BrowserId>,
    pub created_at: std::time::Instant,
}

pub(crate) struct BrowserState {
    pub browser: Browser,
    pub metadata: BrowserMetadata,
    #[allow(dead_code)]
    pub request_context: Option<RequestContext>,
}

pub(crate) struct BrowserRegistry {
    browsers: HashMap<BrowserId, BrowserState>,
    lookup: HashMap<i32, BrowserId>,
    next_id: u32,
    shutdown_signal: ShutdownSignal,
}

impl BrowserRegistry {
    pub fn new(shutdown_signal: ShutdownSignal) -> Self {
        Self {
            browsers: HashMap::new(),
            lookup: HashMap::new(),
            next_id: 1,
            shutdown_signal,
        }
    }

    pub fn register(
        &mut self,
        browser: Browser,
        browser_type: BrowserType,
        parent_id: Option<BrowserId>,
    ) -> BrowserId {
        self.register_with_context(browser, browser_type, parent_id, None)
    }

    pub fn register_with_context(
        &mut self,
        browser: Browser,
        browser_type: BrowserType,
        parent_id: Option<BrowserId>,
        request_context: Option<RequestContext>,
    ) -> BrowserId {
        let id = BrowserId(self.next_id);
        self.next_id += 1;
        let cef_id = browser.identifier();
        let state = BrowserState {
            browser,
            metadata: BrowserMetadata {
                id,
                browser_type,
                parent_id,
                opener_id: None,
                created_at: std::time::Instant::now(),
            },
            request_context,
        };
        debug!("[BrowserRegistry] registered browser {} (type={:?})", id.0, browser_type);
        self.lookup.insert(cef_id, id);
        self.browsers.insert(id, state);
        id
    }

    pub fn unregister(&mut self, id: BrowserId) {
        if let Some(state) = self.browsers.remove(&id) {
            let cef_id = state.browser.identifier();
            self.lookup.remove(&cef_id);
            debug!("[BrowserRegistry] unregistered browser {}", id.0);
            if self.browsers.is_empty() {
                debug!("[BrowserRegistry] last browser removed, signaling shutdown");
                self.shutdown_signal.request_shutdown();
            }
        }
    }

    pub fn count(&self) -> usize {
        self.browsers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.browsers.is_empty()
    }

    pub fn get(&self, id: BrowserId) -> Option<&BrowserState> {
        self.browsers.get(&id)
    }

    #[allow(dead_code)]
    pub fn get_mut(&mut self, id: BrowserId) -> Option<&mut BrowserState> {
        self.browsers.get_mut(&id)
    }

    pub fn find_id_by_browser(&self, browser: &Browser) -> Option<BrowserId> {
        self.lookup.get(&browser.identifier()).copied()
    }

    pub fn set_opener(&mut self, id: BrowserId, opener_id: Option<BrowserId>) {
        if let Some(state) = self.browsers.get_mut(&id) {
            state.metadata.opener_id = opener_id;
        }
    }

    pub fn browser_parent(&self, id: BrowserId) -> Option<BrowserId> {
        self.browsers.get(&id).and_then(|s| s.metadata.parent_id)
    }

    pub fn browser_opener(&self, id: BrowserId) -> Option<BrowserId> {
        self.browsers.get(&id).and_then(|s| s.metadata.opener_id)
    }

    pub fn children_of(&self, parent_id: BrowserId) -> Vec<BrowserId> {
        self.browsers
            .iter()
            .filter(|(_, s)| s.metadata.parent_id == Some(parent_id))
            .map(|(id, _)| *id)
            .collect()
    }

    #[allow(dead_code)]
    pub fn by_type(&self, browser_type: BrowserType) -> Vec<BrowserId> {
        self.browsers
            .iter()
            .filter(|(_, s)| s.metadata.browser_type == browser_type)
            .map(|(id, _)| *id)
            .collect()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&BrowserId, &BrowserState)> {
        self.browsers.iter()
    }
}
