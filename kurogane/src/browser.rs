//! Browser-process lifecycle handling.

use cef::*;
use std::cell::RefCell;
use std::sync::Arc;
use std::time::Duration;

use crate::runtime::RuntimeServices;
use crate::spec::{RuntimeSpec, RuntimeMode};
use crate::browser_registry::BrowserType;
use crate::client::KuroganeClient;
use crate::window::KuroganeWindowDelegate;
use crate::app::{PumpRequest, SecondInstance};
use crate::debug;

wrap_browser_process_handler! {
    pub struct KuroganeBrowserProcessHandler {
        services: Arc<RuntimeServices>,
        spec: RuntimeSpec,

        // Keep factories alive for the browser lifetime; RefCell for interior mutability
        scheme_factories: RefCell<Vec<SchemeHandlerFactory>>,

        // Given to every browser Chromium opens on its own
        chrome_ui_client: Client,
    }

    impl BrowserProcessHandler {
        fn on_context_initialized(&self) {
            debug!("on_context_initialized called");

            // Prevent Chromium from restoring the previous session before creating a window
            start_without_restoring_session();

            // Dispatch to lifecycle delegates first
            for delegate in &self.spec.delegates {
                delegate.on_context_initialized();
            }

            // Register once per request context
            if self.scheme_factories.borrow().is_empty() {
                let mut factories = std::mem::take(&mut *self.scheme_factories.borrow_mut());
                let global = request_context_get_global_context().unwrap();

                // Register `app://` only when serving local assets; URL mode (App::url) has
                // no asset root or scheme handler.
                if let Some(root) = &self.spec.asset_root {
                    debug!("Registering scheme handler factory for app://");
                    let mut factory = crate::scheme::AppSchemeHandlerFactory::new(root.clone());
                    let result = global.register_scheme_handler_factory(
                        Some(&CefString::from("app")),
                        Some(&CefString::from("app")),
                        Some(&mut factory),
                    );
                    debug!("register app:// scheme handler factory result: {result}");
                    factories.push(factory);
                }

                // User-registered custom schemes are served on any host
                for scheme in &self.spec.scheme_handlers {
                    debug!("Registering scheme handler factory for {}://", scheme.name);
                    let mut factory =
                        crate::scheme::CustomSchemeHandlerFactory::new(scheme.handler.clone());
                    let result = global.register_scheme_handler_factory(
                        Some(&CefString::from(scheme.name.as_str())),
                        Some(&CefString::from("")),
                        Some(&mut factory),
                    );
                    debug!(
                        "register {}:// scheme handler factory result: {result}",
                        scheme.name
                    );
                    factories.push(factory);
                }

                // Store so CEF never calls freed memory
                *self.scheme_factories.borrow_mut() = factories;
            }

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
                    KuroganeClient::new(self.services.clone(), BrowserType::Main)
                })
            };

            // Embedded mode delegates window creation to the host application which embeds CEF as a child
            // Skip browser/window creation in on_context_initialized; only register scheme handlers
            if matches!(self.spec.mode, RuntimeMode::Embedded) {
                debug!("Embedded mode; skipping window creation");
                return;
            }

            let url = CefString::from(self.spec.start_url.as_str());

            debug!("Creating main browser with URL: {}", url.to_string());

            debug!("Creating BrowserView");

            let window_id = {
                let mut reg = self.services.window_registry.lock().unwrap();
                reg.allocate_id()
            };

            let mut bv_delegate = crate::window::KuroganeBrowserViewDelegate::new(
                self.services.browser_registry.clone(),
                self.services.window_registry.clone(),
                Some(window_id),
            );

            let browser_view = match browser_view_create(
                Some(&mut client),
                Some(&url),
                Some(&Default::default()),
                None, None,
                Some(&mut bv_delegate),
            ) {
                Some(view) => view,
                None => {
                    eprintln!("kurogane: browser_view_create failed; no window will appear");
                    return;
                }
            };

            debug!("BrowserView created");

            // Create delegate
            let mut delegate = KuroganeWindowDelegate::new(
                window_id,
                browser_view,
                self.services.window_registry.clone(),
                Rect::default(),
                ShowState::NORMAL,
            );

            // Create window
            debug!("Creating top-level window");
            if window_create_top_level(Some(&mut delegate)).is_none() {
                eprintln!("kurogane: window_create_top_level failed; no window will appear");
                return;
            }

            debug!("Top-level window created");
        }

        // CEF asks for this client only when Chromium opens a browser on its
        // own; the application's browsers are created with theirs. Returning
        // none would leave such a browser unmanaged and shutdown would wait
        // until someone closed it by hand
        fn default_client(&self) -> Option<Client> {
            Some(self.chrome_ui_client.clone())
        }

        // CEF runs one instance per profile; a second launch hands its command
        // line to this one and exits. Declining would let CEF open a default
        // Chrome window in this process instead
        fn on_already_running_app_relaunch(
            &self,
            command_line: Option<&mut CommandLine>,
            current_directory: Option<&CefString>,
        ) -> i32 {
            // Chromium brings existing windows to the front before delivering the launch.
            // Keep their current set so windows opened by the handler can be distinguished.
            let windows: Vec<Window> = self
                .services
                .window_registry
                .lock()
                .unwrap()
                .iter()
                .map(|(_, state)| state.window.clone())
                .collect();

            for window in windows {
                if window.is_minimized() != 0 {
                    window.restore();
                }
                window.show();
                window.activate();
            }

            if let (Some(on_second_instance), Some(command_line)) =
                (&self.spec.on_second_instance, command_line)
            {
                on_second_instance(&SecondInstance::from_launch(command_line, current_directory));
            }

            1
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

impl KuroganeBrowserProcessHandler {
    /// Creates the browser process handler.
    ///
    /// CEF requests the handler from multiple threads. Keep one handler and
    /// its state for the lifetime of the process.
    pub(crate) fn create(
        services: Arc<RuntimeServices>,
        spec: RuntimeSpec,
    ) -> BrowserProcessHandler {
        let chrome_ui_client = KuroganeClient::new(services.clone(), BrowserType::ChromeUi);
        Self::new(services, spec, RefCell::new(Vec::new()), chrome_ui_client)
    }
}

/// Chrome's `session.restore_on_startup` value for starting without the last
/// session ([`SessionStartupPref::kPrefValueNewTab`](https://source.chromium.org/chromium/chromium/src/+/main:chrome/browser/prefs/session_startup_pref.h)).
const START_WITHOUT_LAST_SESSION: i32 = 5;

/// Turns off Chromium's "continue where you left off" for this profile.
///
/// Kurogane creates its own windows on each start, so session restore would
/// reopen stale windows after an unclean exit.
fn start_without_restoring_session() {
    let Some(context) = request_context_get_global_context() else {
        return;
    };
    let Some(mut value) = value_create() else {
        return;
    };
    value.set_int(START_WITHOUT_LAST_SESSION);

    let name = CefString::from("session.restore_on_startup");
    // CEF requires a non-null error string
    let mut error = CefString::from("");

    if context.set_preference(Some(&name), Some(&mut value), Some(&mut error)) == 0 {
        eprintln!("kurogane: failed to disable Chromium session restore: {error}");
    }
}
