//! Browser-process lifecycle handling.

use cef::*;
use std::cell::RefCell;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use crate::runtime::RuntimeServices;
use crate::spec::{RuntimeSpec, RuntimeMode};
use crate::client::KuroganeClient;
use crate::window::KuroganeWindowDelegate;
use crate::app::PumpRequest;
use crate::debug;

wrap_browser_process_handler! {
    pub struct KuroganeBrowserProcessHandler {
        services: Arc<RuntimeServices>,
        spec: RuntimeSpec,

        // Keep factory alive for browser lifetime; RefCell for interior mutability
        scheme_factory: RefCell<Option<SchemeHandlerFactory>>,
        default_client_stored: RefCell<Option<Client>>,
    }

    impl BrowserProcessHandler {
        fn on_context_initialized(&self) {
            debug!("on_context_initialized called");

            // Dispatch to lifecycle delegates first
            for delegate in &self.spec.delegates {
                delegate.on_context_initialized();
            }

            // Register once per request context
            if self.scheme_factory.borrow().is_none() {
                debug!("Registering scheme handler factory for app://");

                // Only register the app:// scheme when serving local assets.
                // In URL mode (App::url), there is no asset root and no scheme handler.
                if let Some(root) = &self.spec.asset_root {
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

            let is_closing = Arc::new(AtomicBool::new(false));

            // Check if any delegate provides a custom default client
            let mut client: Client = {
                let mut delegate_client = None;
                for delegate in &self.spec.delegates {
                    if let Some(c) = delegate.default_client() {
                        delegate_client = Some(c);
                        break;
                    }
                }
                delegate_client.unwrap_or_else(|| {
                    KuroganeClient::new(self.services.clone(), is_closing.clone())
                })
            };

            // Store for subsequent default_client calls
            *self.default_client_stored.borrow_mut() = Some(client.clone());

            // Embedded mode delegates window creation to the host application which embeds CEF as a child
            // Skip browser/window creation in on_context_initialized; only register scheme handlers
            if matches!(self.spec.mode, RuntimeMode::Embedded) {
                debug!("Embedded mode; skipping window creation");
                return;
            }

            let url = CefString::from(self.spec.start_url.as_str());

            debug!("Creating main browser with URL: {}", url.to_string());

            debug!("Creating BrowserView");

            let mut bv_delegate = crate::window::KuroganeBrowserViewDelegate::new(
                self.services.browser_registry.clone(),
                self.services.window_registry.clone(),
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
                let mut reg = self.services.window_registry.lock().unwrap();
                reg.allocate_id()
            };

            let mut delegate = KuroganeWindowDelegate::new(
                window_id,
                browser_view,
                self.services.window_registry.clone(),
                Rect::default(),
                ShowState::NORMAL,
                is_closing,
            );

            // Create window
            debug!("Creating top-level window");
            let _window = window_create_top_level(Some(&mut delegate))
                .expect("unrecoverable: window_create_top_level failed");

            debug!("Top-level window created");
        }

        fn default_client(&self) -> Option<Client> {
            self.default_client_stored.borrow().clone()
        }

        fn on_schedule_message_pump_work(&self, delay_ms: i64) {
            if let Some(ref scheduler) = self.spec.scheduler {
                let request = if delay_ms <= 0 {
                    PumpRequest::Now
                } else {
                    PumpRequest::After(Duration::from_millis(delay_ms as u64))
                };
                scheduler(request);
            }
        }
    }
}
