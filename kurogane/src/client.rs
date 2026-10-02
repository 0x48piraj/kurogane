//! Browser client implementation.

use cef::*;
use tracing::{debug, warn};
use crate::runtime::AppHandle;
use crate::browser_registry::BrowserType;
use crate::chrome_commands::KuroganeCommandHandler;
use crate::ipc::FrameId;
use crate::new_window::{self, NewWindowRequest, Outcome};
use crate::window::{Placement, PopupGeometry, open_browser_window};

//
// LifeSpanHandler
//
wrap_life_span_handler! {
    pub struct KuroganeLifeSpanHandler {
        app: AppHandle,
        // What the browsers of this client are, popups aside
        browser_type: BrowserType,
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
            }
            0 // Allow the popup
        }

        fn on_before_popup_aborted(&self, browser: Option<&mut Browser>, popup_id: i32) {
            let mut reg = self.app.registry();
            let opener = browser.and_then(|browser| reg.browsers.find_id_by_browser(browser));
            if let Some(state) = opener.and_then(|id| reg.browsers.get_mut(id)) {
                state.pending_popups.abort(popup_id);
            }
        }

        // DevTools that Chrome's own command opens gets a client of its own
        // too; BrowserHandle::show_devtools does not come through here.
        fn on_before_dev_tools_popup(
            &self,
            _browser: Option<&mut Browser>,
            _window_info: Option<&mut WindowInfo>,
            client: Option<&mut Option<Client>>,
            _settings: Option<&mut BrowserSettings>,
            _extra_info: Option<&mut Option<DictionaryValue>>,
            _use_default_window: Option<&mut i32>,
        ) {
            own_client(client, &self.app, self.browser_type);
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

            // The browser and its window's link go in one update; the guard
            // ends with this statement, before anything below calls CEF
            let closed = self.app.registry().browser_closed(browser);
            let Some(closed) = closed else {
                return;
            };
            debug!("Browser {} destroyed", closed.id.as_u32());

            #[cfg(target_os = "macos")]
            crate::platform::embed::forget_view(closed.id);

            for straggler in closed.stragglers {
                if let Some(host) = straggler.host() {
                    host.close_browser(1);
                }
            }

            // Cancel any pending async handlers for this browser
            self.app.router().cancel_all_for_browser(closed.id);

            if closed.last {
                self.app.all_browsers_closed();
            }
        }
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
/// popup or DevTools browser (its opener's).
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
        *client = Some(KuroganeClient::new(app.clone(), browser_type));
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
                Outcome::Open => {
                    let placement = Placement::Main {
                        bounds: Rect::default(),
                        show_state: ShowState::NORMAL,
                    };
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
// CLIENT
//
wrap_client! {
    pub struct KuroganeClient {
        app: AppHandle,
        // What the browsers of this client are, popups aside
        browser_type: BrowserType,
    }

    impl Client {
        fn command_handler(&self) -> Option<CommandHandler> {
            Some(KuroganeCommandHandler::new())
        }

        fn load_handler(&self) -> Option<LoadHandler> {
            Some(KuroganeLoadHandler::new(self.app.clone()))
        }

        // Only OnOpenURLFromTab is answered; every other method keeps CEF's
        // default, which cef-rs's defaults return
        fn request_handler(&self) -> Option<RequestHandler> {
            Some(KuroganeRequestHandler::new(self.app.clone()))
        }

        fn life_span_handler(&self) -> Option<LifeSpanHandler> {
            Some(KuroganeLifeSpanHandler::new(self.app.clone(), self.browser_type))
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
                Ok(false) => 0,
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
        let opener = KuroganeClient::new(app.clone(), BrowserType::Main);

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
}
