//! Browser client implementation.

use cef::*;
use crate::debug;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use crate::runtime::RuntimeServices;
use crate::browser_registry::{BrowserRegistry, BrowserType};
use crate::chrome_commands::KuroganeCommandHandler;
use crate::ipc::{FrameId, IpcRouter};
use crate::window::PopupGeometry;

//
// LifeSpanHandler
//
wrap_life_span_handler! {
    pub struct KuroganeLifeSpanHandler {
        browser_registry: Arc<Mutex<BrowserRegistry>>,
        is_closing: Arc<AtomicBool>,
        router: Arc<IpcRouter>,
        // What the browsers of this client are, popups aside
        browser_type: BrowserType,
    }

    impl LifeSpanHandler {
        // Save the requested geometry until CEF creates the popup window.
        fn on_before_popup(
            &self,
            browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            popup_id: i32,
            _target_url: Option<&CefString>,
            _target_frame_name: Option<&CefString>,
            _target_disposition: WindowOpenDisposition,
            _user_gesture: i32,
            popup_features: Option<&PopupFeatures>,
            _window_info: Option<&mut WindowInfo>,
            _client: Option<&mut Option<Client>>,
            _settings: Option<&mut BrowserSettings>,
            _extra_info: Option<&mut Option<DictionaryValue>>,
            _no_javascript_access: Option<&mut i32>,
        ) -> i32 {
            let mut reg = self.browser_registry.lock().unwrap();
            let opener = browser.and_then(|browser| reg.find_id_by_browser(browser));
            if let Some(state) = opener.and_then(|id| reg.get_mut(id)) {
                let requested = popup_features.and_then(PopupGeometry::requested);
                state.pending_popups.push(popup_id, requested);
            }
            0 // Allow the popup
        }

        fn on_before_popup_aborted(&self, browser: Option<&mut Browser>, popup_id: i32) {
            let mut reg = self.browser_registry.lock().unwrap();
            let opener = browser.and_then(|browser| reg.find_id_by_browser(browser));
            if let Some(state) = opener.and_then(|id| reg.get_mut(id)) {
                state.pending_popups.abort(popup_id);
            }
        }

        fn on_after_created(&self, browser: Option<&mut Browser>) {
            let Some(browser) = browser else {
                return;
            };
            debug!("on_after_created cef_id={}", browser.identifier());

            let mut reg = self.browser_registry.lock().unwrap();

            // A popup shares its opener's client. The BrowserView delegate
            // classifies a Views popup exactly; CEF does not promise which of
            // the two sees it first, and the first registers it
            let (browser_type, opener) = match self.browser_type {
                // Whatever Chromium opens on its own stays that kind
                BrowserType::ChromeUi => (BrowserType::ChromeUi, None),
                _ if browser.is_popup() != 0 => {
                    let opener = browser
                        .host()
                        .and_then(|host| reg.find_id_by_cef_id(host.opener_identifier()));
                    (BrowserType::Popup, opener)
                }
                browser_type => (browser_type, None),
            };

            reg.ensure_registered(browser, browser_type, opener);
        }

        fn do_close(&self, _browser: Option<&mut Browser>) -> i32 {
            let reg = self.browser_registry.lock().unwrap();
            if reg.count() == 1 {
                self.is_closing.store(true, Ordering::Release);
            }
            0
        }

        fn on_before_close(&self, browser: Option<&mut Browser>) {
            let Some(browser) = browser else {
                return;
            };
            debug!("on_before_close cef_id={}", browser.identifier());

            let (browser_id, stragglers) = {
                let mut reg = self.browser_registry.lock().unwrap();
                let Some(id) = reg.find_id_by_browser(browser) else {
                    return;
                };
                let was_app_browser = reg
                    .get(id)
                    .is_some_and(|state| state.metadata.browser_type != BrowserType::ChromeUi);

                reg.unregister(id);
                debug!("Browser {} destroyed", id.as_u32());

                if reg.is_empty() {
                    debug!("[BrowserRegistry] last browser removed, quitting message loop");

                    // quit_message_loop() is only meaningful when CEF owns the main loop
                    // In embedded mode the host event loop owns shutdown and this call is effectively a no-op
                    // TODO: Move shutdown coordination behind a single runtime lifecycle abstraction instead of mixing quit_message_loop() and shutdown_signal

                    quit_message_loop();
                }

                // Windows Chromium opened on its own close with the
                // application's last one rather than keep the process running
                let stragglers = if was_app_browser && !reg.has_app_browsers() {
                    reg.chrome_ui_browsers()
                } else {
                    Vec::new()
                };

                (id, stragglers)
            };

            for straggler in stragglers {
                if let Some(host) = straggler.host() {
                    host.close_browser(1);
                }
            }

            // Cancel any pending async handlers for this browser
            self.router.cancel_all_for_browser(browser_id);
        }
    }
}

//
// LOAD HANDLER
//
wrap_load_handler! {
    pub struct KuroganeLoadHandler {
        router: Arc<IpcRouter>,
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
            self.router.clear_for_frame(&FrameId::of(frame));
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
        services: Arc<RuntimeServices>,
        is_closing: Arc<AtomicBool>,
        // What the browsers of this client are, popups aside
        browser_type: BrowserType,
    }

    impl Client {
        fn command_handler(&self) -> Option<CommandHandler> {
            Some(KuroganeCommandHandler::new())
        }

        fn load_handler(&self) -> Option<LoadHandler> {
            Some(KuroganeLoadHandler::new(self.services.router.clone()))
        }

        fn life_span_handler(&self) -> Option<LifeSpanHandler> {
            Some(KuroganeLifeSpanHandler::new(
                self.services.browser_registry.clone(),
                self.is_closing.clone(),
                self.services.router.clone(),
                self.browser_type,
            ))
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
                    let reg = self.services.browser_registry.lock().unwrap();
                    reg.find_id_by_browser(browser)
                };
                crate::ipc::handle_ipc_message(browser, frame, msg, &self.services.router, browser_id)
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
