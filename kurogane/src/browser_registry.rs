use std::collections::HashMap;
use cef::{Browser, DownloadItemCallback, ImplBrowser, RequestContext};
use tracing::debug;
use crate::acl::Origin;
use crate::downloads::{Downloads, SavePrompt};
use crate::context_menu::OpenMenu;
use crate::ipc::FrameId;
use crate::file_dialog::PendingFileDialogs;
use crate::permissions::PendingPermissions;
use crate::window::PendingPopups;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BrowserId(u32);

impl BrowserId {
    pub fn as_u32(&self) -> u32 {
        self.0
    }

    #[cfg(test)]
    pub(crate) fn new(id: u32) -> Self {
        Self(id)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum BrowserType {
    Main,
    Popup,
    DevTools,
    #[allow(dead_code)]
    Osr,
    /// A browser Chromium opened on its own, such as a window restored from an
    /// earlier session. It closes with the application rather than keeping
    /// it running.
    ChromeUi,
}

#[derive(Debug, Clone)]
pub struct BrowserMetadata {
    pub id: BrowserId,
    pub browser_type: BrowserType,
    pub parent_id: Option<BrowserId>,
    pub opener_id: Option<BrowserId>,
    pub created_at: std::time::Instant,
}

pub(crate) struct BrowserEntry {
    pub browser: Browser,
    pub metadata: BrowserMetadata,
    #[allow(dead_code)]
    pub request_context: Option<RequestContext>,
    /// Popups this browser opened that are not shown yet; they go with it
    pub pending_popups: PendingPopups,
    /// Origins let into this browser besides the application's own: by the
    /// application's loads, by the popup it is, and by the navigation hook
    /// (see [`crate::navigation`])
    admitted: Vec<Origin>,
    /// Popups this browser's page was allowed, by popup id, with the origin
    /// each was opened to: the popup's own navigation there takes the entry
    /// and lets that origin into the popup
    popup_origins: Vec<(i32, Origin)>,
    /// This browser's downloads Kurogane holds: asking the user, one Save
    /// As dialog at a time, or refused; they go with the browser
    pub downloads: Downloads<SavePrompt, DownloadItemCallback>,
    /// This browser's permission requests waiting for the application's
    /// answer (see [`crate::permissions`]); its close denies them
    pub permissions: PendingPermissions,
    /// This browser's file dialogs waiting for the application's answer
    /// (see [`crate::file_dialog`]); its close cancels them
    pub file_dialogs: PendingFileDialogs,
    /// The last context menu this browser showed: what its items run
    /// (see [`crate::context_menu`])
    pub context_menu: Option<OpenMenu>,
    /// The frames whose document's own origin is opaque (a sandboxed
    /// document), as their renderer reports them
    /// ([`crate::context_menu::opaque_document`])
    pub opaque_documents: Vec<FrameId>,
}

impl BrowserEntry {
    /// Whether `origin` was let into this browser. An opaque origin never is.
    pub(crate) fn admits(&self, origin: &Origin) -> bool {
        !origin.is_opaque() && self.admitted.contains(origin)
    }

    /// Lets `origin` into this browser; an opaque one is not let in.
    pub(crate) fn admit(&mut self, origin: Origin) {
        if !origin.is_opaque() && !self.admitted.contains(&origin) {
            self.admitted.push(origin);
        }
    }

    /// Records that the popup `popup_id`, which this browser's page asked
    /// for, opens to `origin`.
    pub(crate) fn opens_popup(&mut self, popup_id: i32, origin: Origin) {
        self.popup_origins.push((popup_id, origin));
    }

    /// Forgets the popup `popup_id`, which CEF gave up on.
    pub(crate) fn popup_aborted(&mut self, popup_id: i32) {
        self.popup_origins.retain(|(id, _)| *id != popup_id);
    }

    /// Takes the entry of a popup opened to `origin`, if any.
    pub(crate) fn take_popup_origin(&mut self, origin: &Origin) -> bool {
        match self
            .popup_origins
            .iter()
            .position(|(_, opened)| opened == origin)
        {
            Some(index) => {
                self.popup_origins.remove(index);
                true
            }
            None => false,
        }
    }
}

pub(crate) struct BrowserRegistry {
    browsers: HashMap<BrowserId, BrowserEntry>,
    lookup: HashMap<i32, BrowserId>,
    next_id: u32,
}

impl BrowserRegistry {
    pub fn new() -> Self {
        Self {
            browsers: HashMap::new(),
            lookup: HashMap::new(),
            next_id: 1,
        }
    }

    /// Registers `browser`, or returns its id if it is registered already.
    ///
    /// CEF reports a browser to more than one handler and does not promise
    /// their order, so each registers it and the first one wins. The popup
    /// path then refines it with [`Self::classify`].
    pub fn ensure_registered(
        &mut self,
        browser: &Browser,
        browser_type: BrowserType,
        parent_id: Option<BrowserId>,
    ) -> BrowserId {
        match self.find_id_by_browser(browser) {
            Some(id) => id,
            None => self.insert(browser.clone(), browser_type, parent_id, None),
        }
    }

    /// Sets the type of a registered browser and the browser that opened it.
    pub fn classify(
        &mut self,
        id: BrowserId,
        browser_type: BrowserType,
        parent_id: Option<BrowserId>,
    ) {
        if let Some(state) = self.browsers.get_mut(&id) {
            state.metadata.browser_type = browser_type;
            state.metadata.parent_id = parent_id;
            state.metadata.opener_id = parent_id;
        }
    }

    fn insert(
        &mut self,
        browser: Browser,
        browser_type: BrowserType,
        parent_id: Option<BrowserId>,
        request_context: Option<RequestContext>,
    ) -> BrowserId {
        let id = BrowserId(self.next_id);
        self.next_id += 1;
        let cef_id = browser.identifier();
        let state = BrowserEntry {
            browser,
            metadata: BrowserMetadata {
                id,
                browser_type,
                parent_id,
                opener_id: parent_id,
                created_at: std::time::Instant::now(),
            },
            request_context,
            pending_popups: PendingPopups::default(),
            admitted: Vec::new(),
            popup_origins: Vec::new(),
            downloads: Downloads::default(),
            permissions: PendingPermissions::default(),
            file_dialogs: PendingFileDialogs::default(),
            context_menu: None,
            opaque_documents: Vec::new(),
        };
        debug!(
            "[BrowserRegistry] registered browser {} (type={:?})",
            id.0, browser_type
        );
        self.lookup.insert(cef_id, id);
        self.browsers.insert(id, state);
        id
    }

    pub fn unregister(&mut self, id: BrowserId) {
        if let Some(state) = self.browsers.remove(&id) {
            let cef_id = state.browser.identifier();
            self.lookup.remove(&cef_id);
            debug!("[BrowserRegistry] unregistered browser {}", id.0);
        }
    }

    pub fn count(&self) -> usize {
        self.browsers.len()
    }

    /// Returns whether any browser besides those Chromium opened on its own
    /// is left.
    pub fn has_app_browsers(&self) -> bool {
        self.browsers
            .values()
            .any(|state| state.metadata.browser_type != BrowserType::ChromeUi)
    }

    /// The browsers Chromium opened on its own.
    pub fn chrome_ui_browsers(&self) -> Vec<Browser> {
        self.browsers
            .values()
            .filter(|state| state.metadata.browser_type == BrowserType::ChromeUi)
            .map(|state| state.browser.clone())
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.browsers.is_empty()
    }

    pub fn get(&self, id: BrowserId) -> Option<&BrowserEntry> {
        self.browsers.get(&id)
    }

    pub fn get_mut(&mut self, id: BrowserId) -> Option<&mut BrowserEntry> {
        self.browsers.get_mut(&id)
    }

    pub fn find_id_by_cef_id(&self, cef_id: i32) -> Option<BrowserId> {
        self.lookup.get(&cef_id).copied()
    }

    pub fn find_id_by_browser(&self, browser: &Browser) -> Option<BrowserId> {
        self.find_id_by_cef_id(browser.identifier())
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

    pub fn iter(&self) -> impl Iterator<Item = (&BrowserId, &BrowserEntry)> {
        self.browsers.iter()
    }
}
