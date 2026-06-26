//! Browser client implementation.

use cef::*;
use crate::debug;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use crate::runtime::RuntimeServices;
use crate::browser_registry::{BrowserRegistry, BrowserType};
use crate::window_registry::WindowRegistry;
use crate::ipc::IpcRouter;

//
// LifeSpanHandler
//
wrap_life_span_handler! {
    pub struct KuroganeLifeSpanHandler {
        browser_registry: Arc<Mutex<BrowserRegistry>>,
        window_registry: Arc<Mutex<WindowRegistry>>,
        is_closing: Arc<AtomicBool>,
        router: Arc<IpcRouter>,
    }

    impl LifeSpanHandler {
        fn on_after_created(&self, browser: Option<&mut Browser>) {
            if let Some(b) = browser {
                debug!("on_after_created cef_id={}", b.identifier());

                let mut reg = self.browser_registry.lock().unwrap();

                // Only register if not already registered
                // Popups are registered by BrowserViewDelegate::on_popup_browser_view_created
                if reg.find_id_by_browser(b).is_none() {
                    let clone = b.clone();
                    let id = reg.register(clone, BrowserType::Main, None);

                    // Link this main browser to the main window
                    // which was registered without a browser_id in on_window_created
                    let mut wreg = self.window_registry.lock().unwrap();
                    if let Some(wid) = wreg.link_browser_to_unassigned_window(id) {
                        debug!("[BrowserRegistry] linked browser {} to window {}", id.as_u32(), wid.as_u32());
                    }
                }
            }
        }

        fn do_close(&self, _browser: Option<&mut Browser>) -> i32 {
            let reg = self.browser_registry.lock().unwrap();
            if reg.count() == 1 {
                self.is_closing.store(true, Ordering::Release);
            }
            0
        }

        fn on_before_close(&self, browser: Option<&mut Browser>) {
            debug!("on_before_close called");
            if let Some(b) = browser {
                debug!("on_before_close cef_id={}", b.identifier());
                let browser_id = {
                    let mut reg = self.browser_registry.lock().unwrap();
                    if let Some(id) = reg.find_id_by_browser(b) {
                        reg.unregister(id);
                        debug!("Browser {} destroyed", id.as_u32());
                        if reg.is_empty() {
                            debug!("[BrowserRegistry] last browser removed, quitting message loop");

                            // quit_message_loop() is only meaningful when CEF owns the main loop
                            // In embedded mode the host event loop owns shutdown and this call is effectively a no-op
                            // TODO: Move shutdown coordination behind a single runtime lifecycle abstraction instead of mixing quit_message_loop() and shutdown_signal

                            quit_message_loop();
                        }
                        id
                    } else {
                        return;
                    }
                };
                // Cancel any pending async handlers for this browser
                self.router.cancel_all_for_browser(browser_id);
            }
        }
    }
}

//
// LOAD HANDLER
//
wrap_load_handler! {
    pub struct KuroganeLoadHandler;

    impl LoadHandler {
        fn on_load_start(
            &self,
            _browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            _transition_type: TransitionType,
        ) {
            if let Some(f) = frame {
                let u: CefString = (&f.url()).into();
                debug!("[LoadHandler] START {}", u.to_string());
            }
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
    }

    impl Client {
        fn load_handler(&self) -> Option<LoadHandler> {
            Some(KuroganeLoadHandler::new())
        }

        fn life_span_handler(&self) -> Option<LifeSpanHandler> {
            Some(KuroganeLifeSpanHandler::new(
                self.services.browser_registry.clone(),
                self.services.window_registry.clone(),
                self.is_closing.clone(),
                self.services.router.clone(),
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

            let browser = browser.unwrap();
            let frame = frame.unwrap();
            let msg = message.unwrap();

            // Resolve browser identity from the registry
            let browser_id = {
                let reg = self.services.browser_registry.lock().unwrap();
                reg.find_id_by_browser(browser)
            };

            // Delegate to IPC dispatcher with browser context
            if crate::ipc::handle_ipc_message(browser, frame, msg, &self.services.router, browser_id) {
                return 1;
            }

            0
        }
    }
}

impl Drop for KuroganeClient {
    fn drop(&mut self) {
        debug!("KuroganeClient dropped");
    }
}
