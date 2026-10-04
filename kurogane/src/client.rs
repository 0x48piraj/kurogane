//! Browser client implementation.

use std::path::Path;

use cef::*;
use tracing::{debug, warn};
use crate::runtime::AppHandle;
use crate::browser_registry::{BrowserId, BrowserType};
use crate::chrome_commands::KuroganeCommandHandler;
use crate::context_menu;
use crate::destination::{Outcome, is_blank, is_chromium_page};
use crate::downloads::{self, Answer, DownloadRequest, SavePrompt};
use crate::ipc::FrameId;
use crate::keys::{self, KeyDecision, KeyPress};
use crate::navigation::{self, NavigationRequest};
use crate::new_window::{self, NewWindowRequest};
use crate::permissions::{self, Answer as PermissionAnswer, Pending, PermissionRequest};
use crate::window::{Placement, PopupGeometry, open_browser_window};
use crate::window_closing;

/// A load the application made itself, through CreateBrowser, LoadURL or
/// LoadRequest, and the redirects it leads to (cef_types.h)
const DIRECT_LOAD: u32 = sys::cef_transition_type_t::TT_DIRECT_LOAD_FLAG as u32;
/// Back or forward through the browser's history (cef_types.h)
const FORWARD_BACK: u32 = sys::cef_transition_type_t::TT_FORWARD_BACK_FLAG as u32;

//
// LifeSpanHandler
//
wrap_life_span_handler! {
    pub struct KuroganeLifeSpanHandler {
        app: AppHandle,
        // What the browsers of this client are, popups aside
        browser_type: BrowserType,
        // For DevTools, the browser it inspects
        inspects: Option<BrowserId>,
    }

    impl LifeSpanHandler {
        // Give the popup a client of its own, decide whether it opens, and
        // save the requested geometry of one that does until CEF creates
        // its window.
        fn on_before_popup(
            &self,
            browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            popup_id: i32,
            target_url: Option<&CefString>,
            _target_frame_name: Option<&CefString>,
            _target_disposition: WindowOpenDisposition,
            user_gesture: i32,
            popup_features: Option<&PopupFeatures>,
            _window_info: Option<&mut WindowInfo>,
            client: Option<&mut Option<Client>>,
            _settings: Option<&mut BrowserSettings>,
            _extra_info: Option<&mut Option<DictionaryValue>>,
            _no_javascript_access: Option<&mut i32>,
        ) -> i32 {
            // On every path, a cancelled popup's included
            own_client(client, &self.app, self.browser_type);

            // Before the geometry is saved: CEF reports no abort for a popup
            // cancelled here
            let request = new_window_request(frame, target_url, user_gesture);
            match new_window::decide(&self.app, &request) {
                Outcome::Open => {}
                Outcome::External => {
                    crate::external::open(request.url());
                    return 1;
                }
                Outcome::Refuse => return 1,
            }

            let mut reg = self.app.registry();
            let opener = browser.and_then(|browser| reg.browsers.find_id_by_browser(browser));
            if let Some(state) = opener.and_then(|id| reg.browsers.get_mut(id)) {
                let requested = popup_features.and_then(PopupGeometry::requested);
                state.pending_popups.push(popup_id, requested);
                // Let into the popup by its first navigation (crate::navigation)
                state.opens_popup(popup_id, request.origin().clone());
            }
            0 // Allow the popup
        }

        fn on_before_popup_aborted(&self, browser: Option<&mut Browser>, popup_id: i32) {
            let mut reg = self.app.registry();
            let opener = browser.and_then(|browser| reg.browsers.find_id_by_browser(browser));
            if let Some(state) = opener.and_then(|id| reg.browsers.get_mut(id)) {
                state.pending_popups.abort(popup_id);
                state.popup_aborted(popup_id);
            }
        }

        // Every DevTools window comes through here, Chrome's command and
        // BrowserHandle::show_devtools alike, in a window and in an embedded
        // browser, and gets a client of its own that names the browser it
        // inspects. One opened without a client would otherwise get CEF's
        // default client, Kurogane's for the windows Chromium opens on its
        // own, and register as one of those
        fn on_before_dev_tools_popup(
            &self,
            browser: Option<&mut Browser>,
            _window_info: Option<&mut WindowInfo>,
            client: Option<&mut Option<Client>>,
            _settings: Option<&mut BrowserSettings>,
            _extra_info: Option<&mut Option<DictionaryValue>>,
            _use_default_window: Option<&mut i32>,
        ) {
            let inspects = browser.and_then(|browser| self.app.registry().browsers.find_id_by_browser(browser));
            if let Some(client) = client {
                *client = Some(KuroganeClient::new(self.app.clone(), self.browser_type, inspects));
            }
        }

        fn on_after_created(&self, browser: Option<&mut Browser>) {
            let Some(browser) = browser else {
                return;
            };
            debug!("on_after_created cef_id={}", browser.identifier());

            let mut reg = self.app.registry();

            // A popup has a client of its opener's kind (own_client). The
            // BrowserView delegate classifies a Views popup exactly; CEF does
            // not promise which of the two sees it first, and the first
            // registers it
            let (browser_type, opener) = match self.browser_type {
                // DevTools, with the browser it inspects
                _ if self.inspects.is_some() => (BrowserType::DevTools, self.inspects),
                // Whatever Chromium opens on its own stays that kind
                BrowserType::ChromeUi => (BrowserType::ChromeUi, None),
                _ if browser.is_popup() != 0 => {
                    let opener = browser
                        .host()
                        .and_then(|host| reg.browsers.find_id_by_cef_id(host.opener_identifier()));
                    (BrowserType::Popup, opener)
                }
                browser_type => (browser_type, None),
            };

            reg.browsers.ensure_registered(browser, browser_type, opener);
            drop(reg);

            // A browser CEF was already creating when a mandatory end began
            // (a popup decided before it, a window Chromium opened itself)
            // closes now: none may outlive that end. Registered first, so its
            // close is the one that ends the application
            if self.app.is_ending()
                && let Some(host) = browser.host()
            {
                debug!("closing browser cef_id={}: the application is ending", browser.identifier());
                host.close_browser(1);
            }
        }

        // CEF calls `do_close` only for Alloy-style browsers. In Kurogane, these are
        // browsers created with `create_child_browser` and the popups they open.
        // For an embedded browser, CEF's default asks the host's top-level window
        // to close. Kurogane destroys the browser's own child window instead which
        // completes the close. A popup already lives in a top-level window created
        // by CEF, where the default is right.
        //
        // Linux keeps CEF's default, which closes the browser's X window.
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        fn do_close(&self, browser: Option<&mut Browser>) -> ::std::os::raw::c_int {
            match browser {
                // If the task cannot be posted, fall back to CEF's default.
                Some(browser) if browser.is_popup() == 0 => {
                    crate::platform::embed::destroy_child_window_later(browser).into()
                }
                _ => 0,
            }
        }

        fn on_before_close(&self, browser: Option<&mut Browser>) {
            let Some(browser) = browser else {
                return;
            };
            debug!("on_before_close cef_id={}", browser.identifier());

            // The browser, its window's link and an application window's
            // name go in one update; every way a window closes comes through
            // here, its window still there. The guard ends with this
            // statement, before anything below calls CEF
            let closed = self.app.registry().browser_closed(browser);
            let Some(closed) = closed else {
                return;
            };
            debug!("Browser {} destroyed", closed.id.as_u32());

            for waiting in closed.waiting_permissions {
                waiting.answer(false);
            }

            #[cfg(target_os = "macos")]
            crate::platform::embed::forget_view(closed.id);

            for straggler in closed.stragglers {
                if let Some(host) = straggler.host() {
                    host.close_browser(1);
                }
            }

            // Cancel any pending async handlers for this browser
            self.app.router().cancel_all_for_browser(closed.id);

            // Before the last browser ends the application
            if let Some(closing) = closed.closing {
                window_closing::report(&self.app, closing);
            }

            if closed.last {
                self.app.all_browsers_closed();
            }
        }
    }
}

/// The transition of `request`, as the bits CEF returns: the source in the
/// low byte, qualifier flags above it (cef_types.h, cef_transition_type_t).
///
/// cef-rs binds that type as a Rust enum, but CEF returns a source ORed with
/// qualifiers, which no variant names, and holding such a value in the enum
/// is undefined behaviour. So `ImplRequest::transition_type` is never
/// called: CEF's function is called as returning the integer it does.
fn transition(request: &Request) -> u32 {
    let raw = request.get_raw();
    type GetTransitionType = unsafe extern "C" fn(*mut sys::_cef_request_t) -> i32;
    // SAFETY: `raw` is the live request CEF passed to this callback, borrowed
    // as the call's `self` like every method cef-rs calls (no reference is
    // taken or released). The slot's C type returns cef_transition_type_t, a
    // C enum: a 32-bit integer on every platform CEF supports, so reading it
    // through an `i32` return type is the same call; only the Rust enum
    // cef-rs declares cannot hold the value.
    unsafe {
        let Some(get) = (*raw).get_transition_type else {
            return 0;
        };
        let get: GetTransitionType = std::mem::transmute(get);
        get(raw) as u32
    }
}

/// A page's request, made in `frame`, for a window showing `target_url`.
fn new_window_request(
    frame: Option<&mut Frame>,
    target_url: Option<&CefString>,
    user_gesture: i32,
) -> NewWindowRequest {
    let opener_url = frame.map(|frame| CefString::from(&frame.url()).to_string());
    NewWindowRequest::new(
        target_url.map(CefString::to_string).unwrap_or_default(),
        opener_url.as_deref().unwrap_or_default(),
        user_gesture != 0,
    )
}

/// Whether a navigation of `disposition` asks for a window of its own: a
/// new tab or window, as a link clicked with Ctrl (Cmd on macOS), the middle
/// button or Shift asks. The current tab, a download (Alt) and an ignored
/// action are not.
fn opens_new_window(disposition: WindowOpenDisposition) -> bool {
    ![
        WindowOpenDisposition::UNKNOWN,
        WindowOpenDisposition::CURRENT_TAB,
        WindowOpenDisposition::SAVE_TO_DISK,
        WindowOpenDisposition::IGNORE_ACTION,
    ]
    .contains(&disposition)
}

/// Puts a new client in `client`, in place of the one CEF passes in for a
/// popup (its opener's). DevTools gets one of its own the same way
/// (`on_before_dev_tools_popup`).
///
/// cef-rs keeps the reference CEF passes with that client when a handler
/// leaves it unchanged, so the opener's client, and the application's state
/// it holds, would never be released. Replacing the client releases that
/// reference. Remove this once cef-rs releases it itself.
fn own_client(client: Option<&mut Option<Client>>, app: &AppHandle, browser_type: BrowserType) {
    // No client stays no client
    if let Some(client) = client
        && client.is_some()
    {
        *client = Some(KuroganeClient::new(app.clone(), browser_type, None));
    }
}

//
// REQUEST HANDLER
//
wrap_request_handler! {
    pub struct KuroganeRequestHandler {
        app: AppHandle,
    }

    impl RequestHandler {
        // Where a page may take the window it is in (crate::navigation)
        fn on_before_browse(
            &self,
            browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            request: Option<&mut Request>,
            user_gesture: i32,
            is_redirect: i32,
        ) -> i32 {
            let (Some(browser), Some(frame), Some(request)) = (browser, frame, request) else {
                return 0;
            };
            let url = CefString::from(&request.url()).to_string();
            let transition = transition(request);
            debug!(
                "[nav] main={} url={url} transition={transition:#010x} gesture={user_gesture} redirect={is_redirect}",
                frame.is_main()
            );
            // Frames inside a page are not guarded: the ACL keeps another
            // origin's frame from the bridge
            if frame.is_main() == 0 {
                return 0;
            }
            let from = CefString::from(&frame.url()).to_string();
            let navigation = NavigationRequest::new(url, &from, user_gesture != 0, is_redirect != 0);
            let origin = navigation.origin().clone();

            // What this browser may show; the guard ends before the hook runs
            let (id, admitted) = {
                let mut reg = self.app.registry();
                let id = reg.browsers.find_id_by_browser(browser);
                let state = id.and_then(|id| reg.browsers.get(id));
                let kind = state.map(|state| state.metadata.browser_type);
                let opener = state.and_then(|state| state.metadata.opener_id);
                // Chromium's own browsers are not the application's to guard
                if matches!(kind, Some(BrowserType::DevTools | BrowserType::ChromeUi)) {
                    return 0;
                }
                // The application's own loads and their redirects; pages
                // cannot set this flag
                if transition & DIRECT_LOAD != 0 {
                    if let Some(state) = id.and_then(|id| reg.browsers.get_mut(id)) {
                        state.admit(origin);
                    }
                    return 0;
                }
                // History holds only pages that were let in
                if transition & FORWARD_BACK != 0 {
                    return 0;
                }
                let admitted = (!origin.is_opaque() && origin == *self.app.app_origin())
                    || is_blank(navigation.url())
                    || is_chromium_page(navigation.url())
                    || state.is_some_and(|state| state.admits(&origin))
                    || (kind == Some(BrowserType::Popup)
                        && opener
                            .and_then(|opener| reg.browsers.get_mut(opener))
                            .is_some_and(|opener| opener.take_popup_origin(&origin)));
                (id, admitted)
            };

            match navigation::decide(&self.app, &navigation, admitted) {
                Outcome::Open => {
                    let mut reg = self.app.registry();
                    if let Some(state) = id.and_then(|id| reg.browsers.get_mut(id)) {
                        state.admit(origin);
                    }
                    0
                }
                Outcome::External => {
                    crate::external::open(navigation.url());
                    1
                }
                Outcome::Refuse => 1,
            }
        }

        // A link opened in a new tab or window comes here, never to
        // OnBeforePopup. Unanswered, Chromium opens it in a tabbed browser
        // window of its own (Chrome style) or in the source browser itself
        // (Alloy style). Kurogane decides it as it decides a popup, and
        // Chromium opens nothing: an allowed page gets an application window
        // of its own, as create_window makes, with no opener, as a new tab
        // has none
        fn on_open_urlfrom_tab(
            &self,
            _browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            target_url: Option<&CefString>,
            target_disposition: WindowOpenDisposition,
            user_gesture: i32,
        ) -> i32 {
            if !opens_new_window(target_disposition) {
                return 0;
            }
            let request = new_window_request(frame, target_url, user_gesture);
            match new_window::decide(&self.app, &request) {
                // A window of the application's own, as create_window's
                // with no options
                Outcome::Open => {
                    let placement = Placement::Main(crate::WindowOptions::new());
                    if let Err(error) = open_browser_window(&self.app, request.url(), placement) {
                        warn!("no window for {}: {error}", request.url());
                    }
                }
                Outcome::External => crate::external::open(request.url()),
                Outcome::Refuse => {}
            }
            1
        }
    }
}

//
// KEYBOARD HANDLER
//
// cef-rs types the platform's own event differently on each platform, and
// its macro takes no attribute on a parameter, so the handler is written
// once per platform around one body
#[cfg(target_os = "windows")]
wrap_keyboard_handler! {
    pub struct KuroganeKeyboardHandler {
        app: AppHandle,
    }

    impl KeyboardHandler {
        fn on_pre_key_event(
            &self,
            browser: Option<&mut Browser>,
            event: Option<&KeyEvent>,
            _os_event: Option<&mut sys::MSG>,
            is_keyboard_shortcut: Option<&mut i32>,
        ) -> i32 {
            pre_key_event(&self.app, browser, event, is_keyboard_shortcut)
        }
    }
}

#[cfg(target_os = "linux")]
wrap_keyboard_handler! {
    pub struct KuroganeKeyboardHandler {
        app: AppHandle,
    }

    impl KeyboardHandler {
        fn on_pre_key_event(
            &self,
            browser: Option<&mut Browser>,
            event: Option<&KeyEvent>,
            _os_event: Option<&mut sys::XEvent>,
            is_keyboard_shortcut: Option<&mut i32>,
        ) -> i32 {
            pre_key_event(&self.app, browser, event, is_keyboard_shortcut)
        }
    }
}

#[cfg(target_os = "macos")]
wrap_keyboard_handler! {
    pub struct KuroganeKeyboardHandler {
        app: AppHandle,
    }

    impl KeyboardHandler {
        fn on_pre_key_event(
            &self,
            browser: Option<&mut Browser>,
            event: Option<&KeyEvent>,
            _os_event: *mut u8,
            is_keyboard_shortcut: Option<&mut i32>,
        ) -> i32 {
            pre_key_event(&self.app, browser, event, is_keyboard_shortcut)
        }
    }
}

/// A key on its way to a page, before Chromium's own shortcuts see it
/// (crate::keys). Only its press reaches the application's hook; consuming
/// the press drops the key's character and release too.
fn pre_key_event(
    app: &AppHandle,
    browser: Option<&mut Browser>,
    event: Option<&KeyEvent>,
    is_keyboard_shortcut: Option<&mut i32>,
) -> i32 {
    let (Some(browser), Some(event)) = (browser, event) else {
        return 0;
    };
    if event.type_ != KeyEventType::RAWKEYDOWN {
        return 0;
    }
    // The guard ends with the block, before the hook runs
    let id = {
        let reg = app.registry();
        let id = reg.browsers.find_id_by_browser(browser);
        let kind = id
            .and_then(|id| reg.browsers.get(id))
            .map(|state| state.metadata.browser_type);
        // Chromium's own browsers, DevTools' included, are not the application's
        if matches!(kind, Some(BrowserType::DevTools | BrowserType::ChromeUi)) {
            return 0;
        }
        id
    };
    let press = KeyPress::new(
        event.windows_key_code as u32,
        event.modifiers,
        event.character,
        event.focus_on_editable_field != 0,
        id,
    );
    answer_key(keys::decide(app, &press), is_keyboard_shortcut)
}

/// What `on_pre_key_event` answers CEF for `decision`: whether the key is
/// handled, and in `is_keyboard_shortcut` whether it is a shortcut, which
/// CEF holds until the page has seen the key and runs only if the page lets
/// it through.
fn answer_key(decision: KeyDecision, is_keyboard_shortcut: Option<&mut i32>) -> i32 {
    match decision {
        KeyDecision::Consume => 1,
        KeyDecision::Default => 0,
        KeyDecision::PageFirst => {
            if let Some(shortcut) = is_keyboard_shortcut {
                *shortcut = 1;
            }
            0
        }
    }
}

//
// DOWNLOAD HANDLER
//
wrap_download_handler! {
    pub struct KuroganeDownloadHandler {
        app: AppHandle,
    }

    impl DownloadHandler {
        // CEF's own answer, which leaves the decision to OnBeforeDownload;
        // cef-rs's default 0 would cancel every download
        fn can_download(
            &self,
            _browser: Option<&mut Browser>,
            _url: Option<&CefString>,
            _request_method: Option<&CefString>,
        ) -> i32 {
            1
        }

        // Where the file goes (crate::downloads). Returning 0 would let
        // Chromium save it silently into the Downloads folder, so every
        // path answers 1
        fn on_before_download(
            &self,
            browser: Option<&mut Browser>,
            download_item: Option<&mut DownloadItem>,
            suggested_name: Option<&CefString>,
            callback: Option<&mut BeforeDownloadCallback>,
        ) -> i32 {
            let (Some(browser), Some(item), Some(callback)) = (browser, download_item, callback) else {
                return 1;
            };
            let page_url = browser
                .main_frame()
                .map(|frame| CefString::from(&frame.url()).to_string())
                .unwrap_or_default();
            // The guard ends with the call, before the hook runs
            let (id, ask) = app_browser(&self.app, Some(browser));
            let request = DownloadRequest::new(
                CefString::from(&item.url()).to_string(),
                &page_url,
                suggested_name.map(CefString::to_string).unwrap_or_default(),
                CefString::from(&item.mime_type()).to_string(),
                id,
            );
            match downloads::decide(&self.app, &request, ask) {
                // One dialog per browser at a time (downloads::Downloads);
                // the guard ends with the statement, before CEF is called
                Answer::Prompt => {
                    let prompt = SavePrompt {
                        proceed: callback.clone(),
                        name: request.suggested_name().to_owned(),
                    };
                    let now = match id {
                        Some(id) => match self.app.registry().browsers.get_mut(id) {
                            Some(state) => state.downloads.ask(item.id(), prompt),
                            None => Some(prompt),
                        },
                        None => Some(prompt),
                    };
                    if let Some(prompt) = now {
                        ask_where(&self.app, browser, id, item.id(), prompt);
                    }
                }
                Answer::SaveTo(path) => {
                    let path = CefString::from(path.to_string_lossy().as_ref());
                    callback.cont(Some(&path), 0);
                }
                // Never continued, so nothing is written to its place; CEF
                // would keep it pending, and its next update cancels it (a
                // browser never registered keeps it pending until it closes)
                Answer::Refuse => {
                    if let Some(id) = id
                        && let Some(state) = self.app.registry().browsers.get_mut(id)
                    {
                        state.downloads.refuse(item.id());
                    }
                }
            }
            1
        }

        fn on_download_updated(
            &self,
            browser: Option<&mut Browser>,
            download_item: Option<&mut DownloadItem>,
            callback: Option<&mut DownloadItemCallback>,
        ) {
            let Some(item) = download_item else {
                return;
            };
            let path = CefString::from(&item.full_path()).to_string();
            debug!(
                "[download] {} complete={} canceled={} interrupted={} reason={:?} path={path}",
                item.id(),
                item.is_complete(),
                item.is_canceled(),
                item.is_interrupted(),
                item.interrupt_reason(),
            );
            let cancelled = item.is_canceled() != 0;
            let ended = cancelled || item.is_complete() != 0 || item.is_interrupted() != 0;
            // A refused download is cancelled; one asking the user keeps
            // what cancels it, should the user dismiss its dialog
            // (downloads::Downloads). The guard ends with the block
            let cancel = browser.and_then(|browser| {
                let mut reg = self.app.registry();
                let id = reg.browsers.find_id_by_browser(browser)?;
                let state = reg.browsers.get_mut(id)?;
                state.downloads.update(item.id(), callback.as_deref().cloned(), ended)
            });
            if let Some(cancel) = cancel {
                debug!("[download] {} refused: cancelling", item.id());
                cancel.cancel();
            }
        }
    }
}

/// Asks the user where to save download `id` of `browser` with a Save As
/// dialog Kurogane opens itself (downloads::Downloads says why not CEF's), the
/// suggested name and its extension filled in.
fn ask_where(
    app: &AppHandle,
    browser: &Browser,
    browser_id: Option<BrowserId>,
    id: u32,
    prompt: SavePrompt,
) {
    let Some(host) = browser.host() else {
        answered(app, browser_id, id, false);
        return;
    };
    let mut filters = CefStringList::new();
    if let Some(extension) = Path::new(&prompt.name).extension() {
        filters.append(&format!(".{}", extension.to_string_lossy()));
    }
    let name = CefString::from(prompt.name.as_str());
    let mut answer = KuroganeSaveDialog::new(
        app.clone(),
        browser_id,
        id,
        prompt.proceed,
        prompt.name,
        page_of(browser),
    );
    host.run_file_dialog(
        FileDialogMode::SAVE,
        None,
        Some(&name),
        Some(&mut filters),
        Some(&mut answer),
    );
}

/// The document `browser` shows, by its main frame, which a navigation to
/// another document replaces.
fn page_of(browser: &Browser) -> String {
    browser
        .main_frame()
        .map(|frame| CefString::from(&frame.identifier()).to_string())
        .unwrap_or_default()
}

/// The dialog of download `id` was answered, `saved` with a place: a
/// dismissal cancels the download, and the browser's next waiting download
/// asks. The guard ends with the statement, before CEF is called.
fn answered(app: &AppHandle, browser_id: Option<BrowserId>, id: u32, saved: bool) {
    let (cancel, next) = browser_id
        .and_then(|browser_id| {
            let mut reg = app.registry();
            let state = reg.browsers.get_mut(browser_id)?;
            let (cancel, next) = state.downloads.answered(id, saved);
            Some((cancel, next.map(|next| (state.browser.clone(), next))))
        })
        .unwrap_or_default();
    if let Some(cancel) = cancel {
        cancel.cancel();
    }
    if let Some((browser, (next, prompt))) = next {
        ask_where(app, &browser, browser_id, next, prompt);
    }
}

wrap_run_file_dialog_callback! {
    pub struct KuroganeSaveDialog {
        app: AppHandle,
        browser_id: Option<BrowserId>,
        download: u32,
        proceed: BeforeDownloadCallback,
        name: String,
        // The document the dialog opened over (page_of)
        page: String,
    }

    impl RunFileDialogCallback {
        // The place the user chose, without CEF's own dialog
        fn on_file_dialog_dismissed(&self, file_paths: Option<&mut CefStringList>) {
            let chosen = file_paths.and_then(|paths| {
                // A borrowed list, which the copy does not free
                let paths: *mut sys::_cef_string_list_t = paths.into();
                CefStringList::from(paths).into_iter().next()
            });
            if let Some(path) = &chosen {
                self.proceed.cont(Some(&CefString::from(path.as_str())), 0);
                answered(&self.app, self.browser_id, self.download, true);
                return;
            }
            // Chromium drops a dialog's answer, a chosen place too, once the
            // document it opened over is replaced: then the user is asked
            // again rather than the download cancelled. The guard ends with
            // the statement
            let browser = self
                .browser_id
                .and_then(|id| self.app.registry().browsers.get(id).map(|state| state.browser.clone()));
            match browser {
                Some(browser) if page_of(&browser) != self.page => {
                    debug!("[download] {} asked again: its page was replaced", self.download);
                    let prompt = SavePrompt {
                        proceed: self.proceed.clone(),
                        name: self.name.clone(),
                    };
                    ask_where(&self.app, &browser, self.browser_id, self.download, prompt);
                }
                _ => answered(&self.app, self.browser_id, self.download, false),
            }
        }
    }
}

//
// PERMISSION HANDLER
//
wrap_permission_handler! {
    pub struct KuroganePermissionHandler {
        app: AppHandle,
    }

    impl PermissionHandler {
        // Chromium's prompt: for multiple downloads the download policy's
        // (crate::downloads), every other the application's
        // (crate::permissions). Every prompt is answered: returning 0 would
        // show Chromium's own prompt in a window and leave the request
        // pending forever in an embedded browser
        fn on_show_permission_prompt(
            &self,
            browser: Option<&mut Browser>,
            prompt_id: u64,
            requesting_origin: Option<&CefString>,
            requested_permissions: u32,
            callback: Option<&mut PermissionPromptCallback>,
        ) -> i32 {
            let origin = requesting_origin.map(CefString::to_string).unwrap_or_default();
            debug!("[permission] prompt {prompt_id} for {origin}: {requested_permissions:#x}");
            // CEF always passes one
            let Some(callback) = callback else {
                return 0;
            };
            if downloads::grants_prompt(requested_permissions) {
                debug!("multiple downloads granted to {origin}");
                callback.cont(permissions::prompt_result(true));
                return 1;
            }
            let (id, ask) = app_browser(&self.app, browser.as_deref());
            let request = PermissionRequest::prompt(&self.app, &origin, requested_permissions, id);
            let allow = match permissions::decide(&self.app, &request, ask) {
                PermissionAnswer::Allow => true,
                PermissionAnswer::Deny => false,
                PermissionAnswer::Later => {
                    let pending = Pending::Prompt {
                        callback: callback.clone(),
                        prompt: prompt_id,
                    };
                    match permissions::hold(&self.app, id, request.id(), pending) {
                        Ok(()) => return 1,
                        Err(_) => false,
                    }
                }
            };
            callback.cont(permissions::prompt_result(allow));
            1
        }

        // A camera, a microphone or the screen (crate::permissions). Every
        // request is answered, as every prompt is
        fn on_request_media_access_permission(
            &self,
            browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            requesting_origin: Option<&CefString>,
            requested_permissions: u32,
            callback: Option<&mut MediaAccessCallback>,
        ) -> i32 {
            let origin = requesting_origin.map(CefString::to_string).unwrap_or_default();
            debug!("[permission] media for {origin}: {requested_permissions:#x}");
            // CEF always passes one
            let Some(callback) = callback else {
                return 0;
            };
            let (id, ask) = app_browser(&self.app, browser.as_deref());
            let request = PermissionRequest::media(&self.app, &origin, requested_permissions, id);
            let allow = match permissions::decide(&self.app, &request, ask) {
                PermissionAnswer::Allow => true,
                PermissionAnswer::Deny => false,
                PermissionAnswer::Later => {
                    let pending = Pending::Media {
                        callback: callback.clone(),
                        requested: requested_permissions,
                        frame: frame.as_deref().cloned(),
                    };
                    match permissions::hold(&self.app, id, request.id(), pending) {
                        Ok(()) => return 1,
                        Err(_) => false,
                    }
                }
            };
            callback.cont(if allow { requested_permissions } else { 0 });
            1
        }

        // Chromium took its prompt down: answered, or gone with its page,
        // when a request waiting for the application's answer goes too
        fn on_dismiss_permission_prompt(
            &self,
            browser: Option<&mut Browser>,
            prompt_id: u64,
            result: PermissionRequestResult,
        ) {
            debug!("[permission] prompt {prompt_id} ended: {result:?}");
            // The guard ends with the block; what it took goes after it
            let ended = browser.and_then(|browser| {
                let mut reg = self.app.registry();
                let id = reg.browsers.find_id_by_browser(browser)?;
                reg.browsers.get_mut(id)?.permissions.prompt_ended(prompt_id)
            });
            drop(ended);
        }
    }
}

/// The id of `browser` and whether it is the application's: Chromium's own
/// browsers, DevTools' included, are not, and their requests never reach
/// the application's hooks.
fn app_browser(app: &AppHandle, browser: Option<&Browser>) -> (Option<BrowserId>, bool) {
    let reg = app.registry();
    let id = browser.and_then(|browser| reg.browsers.find_id_by_browser(browser));
    let kind = id
        .and_then(|id| reg.browsers.get(id))
        .map(|state| state.metadata.browser_type);
    (
        id,
        !matches!(kind, Some(BrowserType::DevTools | BrowserType::ChromeUi)),
    )
}

//
// CONTEXT MENU HANDLER
//
wrap_context_menu_handler! {
    pub struct KuroganeContextMenuHandler {
        app: AppHandle,
    }

    impl ContextMenuHandler {
        // Kurogane's menu replaces Chromium's (crate::context_menu), in the
        // application's browsers; DevTools' keep their own
        fn on_before_context_menu(
            &self,
            browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            params: Option<&mut ContextMenuParams>,
            model: Option<&mut MenuModel>,
        ) {
            let (Some(browser), Some(params), Some(model)) = (browser, params, model) else {
                return;
            };
            let (id, ask) = app_browser(&self.app, Some(browser));
            if ask {
                context_menu::build(&self.app, browser, frame.as_deref(), id, params, model);
            }
        }

        // Only what Kurogane put in the menu runs
        fn on_context_menu_command(
            &self,
            browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            _params: Option<&mut ContextMenuParams>,
            command_id: i32,
            _event_flags: EventFlags,
        ) -> i32 {
            let Some(browser) = browser else {
                return 1;
            };
            let (id, ask) = app_browser(&self.app, Some(browser));
            if !ask {
                return 0;
            }
            context_menu::chosen(&self.app, browser, id, command_id) as i32
        }
    }
}

//
// LOAD HANDLER
//
wrap_load_handler! {
    pub struct KuroganeLoadHandler {
        app: AppHandle,
    }

    impl LoadHandler {
        fn on_load_start(
            &self,
            _browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            _transition_type: TransitionType,
        ) {
            let Some(frame) = frame else {
                return;
            };
            let u: CefString = (&frame.url()).into();
            debug!("[LoadHandler] START {}", u.to_string());
            // Reset state when the frame loads a new document
            self.app.router().clear_for_frame(&FrameId::of(frame));
        }

        fn on_load_end(
            &self,
            _browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            http_status_code: i32,
        ) {
            if let Some(f) = frame {
                let u: CefString = (&f.url()).into();
                debug!("[LoadHandler] END {} status={}", u.to_string(), http_status_code);
            }
        }

        fn on_load_error(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            error_code: Errorcode,
            error_text: Option<&CefString>,
            failed_url: Option<&CefString>,
        ) {
            let err = error_text.map(|s| s.to_string()).unwrap_or_default();
            let url = failed_url.map(|s| s.to_string()).unwrap_or_default();
            debug!("[LoadHandler] ERROR {:?} '{}' {}", error_code, err, url);
        }
    }
}

//
// DISPLAY HANDLER
//
// Every other method keeps CEF's default, which cef-rs's defaults return
wrap_display_handler! {
    pub struct KuroganeDisplayHandler {
        app: AppHandle,
    }

    impl DisplayHandler {
        // A window takes its page's title, as a browser tab does, unless the
        // application's options fixed one (crate::window). CEF never titles
        // a Views window by itself
        fn on_title_change(&self, browser: Option<&mut Browser>, title: Option<&CefString>) {
            let (Some(browser), Some(title)) = (browser, title) else {
                return;
            };
            // The guard ends with the statement, before the window is titled
            let window = {
                let reg = self.app.registry();
                reg.browsers
                    .find_id_by_browser(browser)
                    .and_then(|id| reg.windows.window_id_for_browser(id))
                    .and_then(|id| reg.windows.get(id))
                    .filter(|state| state.follows_title)
                    .map(|state| state.window.clone())
            };
            if let Some(window) = window {
                window.set_title(Some(title));
            }
        }
    }
}

//
// CLIENT
//
wrap_client! {
    pub struct KuroganeClient {
        app: AppHandle,
        // What the browsers of this client are, popups aside
        browser_type: BrowserType,
        // For DevTools, the browser it inspects (on_before_dev_tools_popup)
        inspects: Option<BrowserId>,
    }

    impl Client {
        fn command_handler(&self) -> Option<CommandHandler> {
            Some(KuroganeCommandHandler::new(self.app.clone()))
        }

        fn display_handler(&self) -> Option<DisplayHandler> {
            Some(KuroganeDisplayHandler::new(self.app.clone()))
        }

        fn download_handler(&self) -> Option<DownloadHandler> {
            Some(KuroganeDownloadHandler::new(self.app.clone()))
        }

        fn load_handler(&self) -> Option<LoadHandler> {
            Some(KuroganeLoadHandler::new(self.app.clone()))
        }

        fn permission_handler(&self) -> Option<PermissionHandler> {
            Some(KuroganePermissionHandler::new(self.app.clone()))
        }

        fn context_menu_handler(&self) -> Option<ContextMenuHandler> {
            Some(KuroganeContextMenuHandler::new(self.app.clone()))
        }

        // Only for an application that asks to see keys
        fn keyboard_handler(&self) -> Option<KeyboardHandler> {
            let wanted = self.app.hooks().is_some_and(|hooks| hooks.key.is_some());
            wanted.then(|| KuroganeKeyboardHandler::new(self.app.clone()))
        }

        // Only OnOpenURLFromTab is answered; every other method keeps CEF's
        // default, which cef-rs's defaults return
        fn request_handler(&self) -> Option<RequestHandler> {
            Some(KuroganeRequestHandler::new(self.app.clone()))
        }

        fn life_span_handler(&self) -> Option<LifeSpanHandler> {
            Some(KuroganeLifeSpanHandler::new(self.app.clone(), self.browser_type, self.inspects))
        }

        fn on_process_message_received(
            &self,
            browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            source_process: ProcessId,
            message: Option<&mut ProcessMessage>,
        ) -> i32 {
            // Only handle messages from renderer
            if source_process != ProcessId::RENDERER {
                return 0;
            }

            let (Some(browser), Some(frame), Some(msg)) = (browser, frame, message) else {
                debug!("[IPC Browser] message without a browser, frame or body");
                return 0;
            };

            // Renderer-controlled bytes drive everything below; a panic must
            // not unwind across this CEF callback and abort the process
            let handled = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                // Resolve browser identity from the registry
                let browser_id = {
                    let reg = self.app.registry();
                    reg.browsers.find_id_by_browser(browser)
                };
                crate::ipc::handle_ipc_message(&self.app, browser, frame, msg, browser_id)
            }));
            match handled {
                Ok(true) => 1,
                // Not IPC: a document's opacity, which IPC's own messages
                // carry with them, so their path never comes here
                Ok(false) => context_menu::opaque_document(&self.app, browser, frame, msg) as i32,
                Err(_) => {
                    debug!("[IPC Browser] dispatch panicked; message dropped");
                    1
                }
            }
        }
    }
}

impl Drop for KuroganeClient {
    fn drop(&mut self) {
        debug!("KuroganeClient dropped");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_tabs_and_windows_are_new_windows_and_nothing_else_is() {
        for disposition in [
            WindowOpenDisposition::NEW_FOREGROUND_TAB,
            WindowOpenDisposition::NEW_BACKGROUND_TAB,
            WindowOpenDisposition::NEW_WINDOW,
            WindowOpenDisposition::NEW_POPUP,
            WindowOpenDisposition::OFF_THE_RECORD,
            WindowOpenDisposition::SINGLETON_TAB,
        ] {
            assert!(opens_new_window(disposition), "{disposition:?}");
        }
        for disposition in [
            WindowOpenDisposition::CURRENT_TAB,
            WindowOpenDisposition::SAVE_TO_DISK,
            WindowOpenDisposition::IGNORE_ACTION,
            WindowOpenDisposition::UNKNOWN,
        ] {
            assert!(!opens_new_window(disposition), "{disposition:?}");
        }
    }

    #[test]
    fn a_new_browser_gets_a_client_of_its_own() {
        let app = AppHandle::detached();
        let opener = KuroganeClient::new(app.clone(), BrowserType::Main, None);

        // What CEF passes in: the opener's client, with a reference of its own
        let mut passed = Some(opener.clone());
        own_client(Some(&mut passed), &app, BrowserType::Main);
        let own = passed.expect("a client is replaced, not cleared");
        assert_ne!(
            cef::ImplClient::get_raw(&own),
            cef::ImplClient::get_raw(&opener),
            "the new browser gets a client of its own"
        );
        assert!(
            cef::rc::Rc::has_one_ref(&opener),
            "the reference that came with the opener's client is released"
        );

        // CEF passed no client: none is made up
        let mut none: Option<Client> = None;
        own_client(Some(&mut none), &app, BrowserType::Main);
        assert!(none.is_none());
    }

    #[test]
    fn page_first_keys_are_shortcuts_and_the_rest_are_left_as_cef_set_them() {
        for (decision, handled, shortcut) in [
            (KeyDecision::Default, 0, 7),
            (KeyDecision::Consume, 1, 7),
            (KeyDecision::PageFirst, 0, 1),
        ] {
            // 7 stands for whatever CEF passed in
            let mut is_keyboard_shortcut = 7;
            assert_eq!(
                answer_key(decision, Some(&mut is_keyboard_shortcut)),
                handled,
                "{decision:?}"
            );
            assert_eq!(is_keyboard_shortcut, shortcut, "{decision:?}");
            assert_eq!(answer_key(decision, None), handled, "{decision:?}");
        }
    }
}
