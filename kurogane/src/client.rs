//! Browser client implementation.

use cef::*;
use crate::debug;
use std::sync::{Arc, Mutex};
use crate::ipc::IpcDispatcher;
use crate::browser_registry::{BrowserRegistry, BrowserType};
use crate::ShutdownSignal;

//
// LifeSpanHandler
//
wrap_life_span_handler! {
    pub struct KuroganeLifeSpanHandler {
        registry: Arc<Mutex<BrowserRegistry>>,
    }

    impl LifeSpanHandler {
        fn on_after_created(&self, browser: Option<&mut Browser>) {
            if let Some(b) = browser {
                let mut reg = self.registry.lock().unwrap();
                // Only register if not already registered
                // Popups are registered by BrowserViewDelegate::on_popup_browser_view_created
                if reg.find_id_by_browser(b).is_none() {
                    let clone = b.clone();
                    reg.register(clone, BrowserType::Main, None);
                }
            }
        }

        fn on_before_close(&self, browser: Option<&mut Browser>) {
            debug!("on_before_close: Native window destroyed, browser object destroyed");
            if let Some(b) = browser {
                let mut reg = self.registry.lock().unwrap();
                if let Some(id) = reg.find_id_by_browser(b) {
                    reg.unregister(id);
                    debug!("Browser {} destroyed", id.as_u32());
                    if reg.is_empty() {
                        debug!("[BrowserRegistry] last browser removed, quitting message loop");
                        quit_message_loop();
                    }
                }
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
        dispatcher: Arc<IpcDispatcher>,
        shutdown_signal: ShutdownSignal,
        registry: Arc<Mutex<BrowserRegistry>>,
    }

    impl Client {
        fn load_handler(&self) -> Option<LoadHandler> {
            Some(KuroganeLoadHandler::new())
        }

        fn life_span_handler(&self) -> Option<LifeSpanHandler> {
            Some(KuroganeLifeSpanHandler::new(self.registry.clone()))
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
                let reg = self.registry.lock().unwrap();
                reg.find_id_by_browser(browser)
            };

            // Delegate to IPC dispatcher with browser context
            if crate::ipc::handle_ipc_message(browser, frame, msg, &self.dispatcher, browser_id) {
                return 1;
            }

            0
        }
    }
}
