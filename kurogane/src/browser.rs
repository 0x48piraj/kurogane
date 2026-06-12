//! Browser-process lifecycle handling.

use cef::*;
use std::cell::RefCell;
use std::sync::{Arc, Mutex};

use crate::fs::CanonicalRoot;
use crate::client::KuroganeClient;
use crate::ipc::IpcDispatcher;
use crate::ShutdownSignal;
use crate::browser_registry::BrowserRegistry;
use crate::window_registry::WindowRegistry;
use crate::window::KuroganeWindowDelegate;
use crate::debug;

wrap_browser_process_handler! {
    pub struct KuroganeBrowserProcessHandler {
        window_registry: Arc<Mutex<WindowRegistry>>,
        registry: Arc<Mutex<BrowserRegistry>>,
        shutdown_signal: ShutdownSignal,
        start_url: CefString,
        asset_root: Option<CanonicalRoot>,
        dispatcher: Arc<IpcDispatcher>,

        // Keep factory alive for browser lifetime; RefCell for interior mutability
        scheme_factory: RefCell<Option<SchemeHandlerFactory>>,

        // When true, skip browser/window creation in on_context_initialized
        // The host application creates its own window and embeds CEF as a child
        embedded_mode: bool,
    }

    impl BrowserProcessHandler {
        fn on_context_initialized(&self) {
            debug!("Browser context initialized");
            debug!("on_context_initialized called");

            // Register once per request context
            if self.scheme_factory.borrow().is_none() {
                debug!("Registering scheme handler factory for app://");

                // Only register the app:// scheme when serving local assets.
                // In URL mode (App::url), there is no asset root and no scheme handler.
                if let Some(root) = &self.asset_root {
                    // Create factory
                    let mut factory = crate::scheme::AppSchemeHandlerFactory::new(root.clone());

                    // Register the scheme handler factory for app:// URLs
                    let global = request_context_get_global_context().unwrap();

                    let result = global.register_scheme_handler_factory(
                        Some(&CefString::from("app")),
                        Some(&CefString::from("app")),
                        Some(&mut factory),
                    );

                    // Store so CEF never calls freed memory
                    *self.scheme_factory.borrow_mut() = Some(factory);

                    debug!("register_scheme_handler_factory result: {}", result);
                }
            }

            // In embedded mode, the host application creates its own window
            // We only register scheme handlers.
            if self.embedded_mode {
                debug!("Embedded mode; skipping window creation");
                return;
            }

            let mut client = KuroganeClient::new(self.dispatcher.clone(), self.shutdown_signal.clone(), self.registry.clone());
            let url = self.start_url.clone();

            debug!("Creating main browser with URL: {}", url.to_string());

            debug!("Creating BrowserView");

            let mut bv_delegate = crate::window::KuroganeBrowserViewDelegate::new(
                self.registry.clone(),
                self.window_registry.clone(),
            );

            let browser_view = browser_view_create(
                Some(&mut client),
                Some(&url),
                Some(&Default::default()),
                None, None,
                Some(&mut bv_delegate),
            )
            .expect("unrecoverable: browser_view_create failed");

            debug!("BrowserView created");

            // Create delegate
            let window_id = {
                let mut reg = self.window_registry.lock().unwrap();
                reg.allocate_id()
            };

            let mut delegate = KuroganeWindowDelegate::new(
                window_id,
                browser_view,
                self.window_registry.clone(),
            );

            // Create window
            debug!("Creating top-level window");
            let _window = window_create_top_level(Some(&mut delegate))
                .expect("unrecoverable: window_create_top_level failed");

            debug!("Top-level window created");
        }

        fn on_schedule_message_pump_work(&self, delay_ms: i64) {
            debug!("CEF requested pump in {}ms", delay_ms);
        }
    }
}
