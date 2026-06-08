//! Browser-process lifecycle handling.
//! A BrowserProcessHandler exists per request context.
//! We only want one native window per application,
//! so we guard creation using the shared window handle.

use cef::*;
use std::cell::RefCell;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::fs::CanonicalRoot;
use crate::client::KuroganeClient;
use crate::ipc::IpcDispatcher;
use crate::debug;

wrap_browser_process_handler! {
    pub struct KuroganeBrowserProcessHandler {
        window: Arc<Mutex<Option<Window>>>,
        start_url: CefString,
        asset_root: Option<CanonicalRoot>,
        dispatcher: Arc<IpcDispatcher>,

        // Keep factory alive for browser lifetime; RefCell for interior mutability
        scheme_factory: RefCell<Option<SchemeHandlerFactory>>,
        window_creation_started: Arc<AtomicBool>,
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

            // Atomically claim the window creation slot; bail if already taken
            if self.window_creation_started.swap(true, Ordering::SeqCst) { // returns the old value
                debug!("Secondary request context; skipping window creation");
                return;
            }

            let mut client = KuroganeClient::new(self.dispatcher.clone());
            let url = self.start_url.clone();

            debug!("Creating main browser with URL: {}", url.to_string());

            debug!("Creating BrowserView");
            let browser_view = browser_view_create(
                Some(&mut client),
                Some(&url),
                Some(&Default::default()),
                None, None, None,
            )
            .expect("unrecoverable: browser_view_create failed");

            debug!("BrowserView created");

            // Create delegate
            let mut delegate = crate::window::KuroganeWindowDelegate::new(browser_view, self.window.clone());

            // Create window
            debug!("Creating top-level window");
            let window = window_create_top_level(Some(&mut delegate))
                .expect("unrecoverable: window_create_top_level failed");

            debug!("Top-level window created");

            *self.window.lock().unwrap() = Some(window);
        }
    }
}
