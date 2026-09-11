//! Browser client implementation.

use cef::*;
use crate::debug;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use crate::app::ClientAppBrowserDelegate;
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
            if let Some(f) = frame {
                let u: CefString = (&f.url()).into();
                debug!("[LoadHandler] START {}", u.to_string());
            }
            self.router.clear_for_frame();
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

/// The first handler a host delegate supplies, in registration order.
fn host_handler<T>(
    delegates: &[Arc<dyn ClientAppBrowserDelegate>],
    pick: impl Fn(&dyn ClientAppBrowserDelegate) -> Option<T>,
) -> Option<T> {
    delegates.iter().find_map(|d| pick(d.as_ref()))
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
            Some(KuroganeLoadHandler::new(self.services.router.clone()))
        }

        fn life_span_handler(&self) -> Option<LifeSpanHandler> {
            Some(KuroganeLifeSpanHandler::new(
                self.services.browser_registry.clone(),
                self.services.window_registry.clone(),
                self.is_closing.clone(),
                self.services.router.clone(),
            ))
        }

        // Every other handler comes from the host, if any delegate supplies it
        fn audio_handler(&self) -> Option<AudioHandler> {
            host_handler(&self.services.delegates, |d| d.audio_handler())
        }

        fn command_handler(&self) -> Option<CommandHandler> {
            host_handler(&self.services.delegates, |d| d.command_handler())
        }

        fn context_menu_handler(&self) -> Option<ContextMenuHandler> {
            host_handler(&self.services.delegates, |d| d.context_menu_handler())
        }

        fn dialog_handler(&self) -> Option<DialogHandler> {
            host_handler(&self.services.delegates, |d| d.dialog_handler())
        }

        fn display_handler(&self) -> Option<DisplayHandler> {
            host_handler(&self.services.delegates, |d| d.display_handler())
        }

        fn download_handler(&self) -> Option<DownloadHandler> {
            host_handler(&self.services.delegates, |d| d.download_handler())
        }

        fn drag_handler(&self) -> Option<DragHandler> {
            host_handler(&self.services.delegates, |d| d.drag_handler())
        }

        fn find_handler(&self) -> Option<FindHandler> {
            host_handler(&self.services.delegates, |d| d.find_handler())
        }

        fn focus_handler(&self) -> Option<FocusHandler> {
            host_handler(&self.services.delegates, |d| d.focus_handler())
        }

        fn frame_handler(&self) -> Option<FrameHandler> {
            host_handler(&self.services.delegates, |d| d.frame_handler())
        }

        fn jsdialog_handler(&self) -> Option<JsdialogHandler> {
            host_handler(&self.services.delegates, |d| d.jsdialog_handler())
        }

        fn keyboard_handler(&self) -> Option<KeyboardHandler> {
            host_handler(&self.services.delegates, |d| d.keyboard_handler())
        }

        fn permission_handler(&self) -> Option<PermissionHandler> {
            host_handler(&self.services.delegates, |d| d.permission_handler())
        }

        fn print_handler(&self) -> Option<PrintHandler> {
            host_handler(&self.services.delegates, |d| d.print_handler())
        }

        fn render_handler(&self) -> Option<RenderHandler> {
            host_handler(&self.services.delegates, |d| d.render_handler())
        }

        fn request_handler(&self) -> Option<RequestHandler> {
            host_handler(&self.services.delegates, |d| d.request_handler())
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    wrap_command_handler! {
        pub struct Nop {}

        impl CommandHandler {}
    }

    struct Silent;
    impl ClientAppBrowserDelegate for Silent {}

    // Counts how often it was asked: the delegate after the winner must not be.
    struct Supplies(Arc<AtomicUsize>);
    impl ClientAppBrowserDelegate for Supplies {
        fn command_handler(&self) -> Option<CommandHandler> {
            self.0.fetch_add(1, Ordering::Relaxed);
            Some(Nop::new())
        }
    }

    #[test]
    fn first_delegate_supplying_a_handler_wins() {
        let (first, second) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
        let delegates: Vec<Arc<dyn ClientAppBrowserDelegate>> = vec![
            Arc::new(Silent),
            Arc::new(Supplies(first.clone())),
            Arc::new(Supplies(second.clone())),
        ];
        assert!(host_handler(&delegates, |d| d.command_handler()).is_some());
        assert_eq!(first.load(Ordering::Relaxed), 1);
        assert_eq!(second.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn no_delegate_no_handler() {
        let delegates: Vec<Arc<dyn ClientAppBrowserDelegate>> = vec![Arc::new(Silent)];
        assert!(host_handler(&delegates, |d| d.command_handler()).is_none());
    }
}
