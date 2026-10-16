//! Browser-process lifecycle handling.

use tanso::*;
use std::time::Duration;

use crate::runtime::AppHandle;
use crate::spec::{RuntimeSpec, RuntimeMode};
use crate::browser_registry::BrowserType;
use crate::client::KuroganeClient;
use crate::window::{Opening, open_browser_window};
use crate::app::{PumpRequest, SecondInstance};
use tracing::{debug, error};

wrap_browser_process_handler! {
    pub struct KuroganeBrowserProcessHandler {
        app: AppHandle,
        spec: RuntimeSpec,

        // Given to every browser Chromium opens on its own
        chrome_ui_client: Client,
    }

    impl BrowserProcessHandler {
        fn on_register_custom_preferences(
            &self,
            type_: PreferencesType,
            registrar: Option<&mut PreferenceRegistrar>,
        ) {
            crate::chromium_defaults::register_preferences(type_, registrar);
        }

        fn on_context_initialized(&self) {
            debug!("on_context_initialized called");

            // Kurogane's preferences and the application's, before any window
            crate::chromium_defaults::apply_preferences(&self.spec.chromium_prefs);

            // Dispatch to lifecycle delegates first
            for delegate in &self.spec.delegates {
                delegate.on_context_initialized();
            }

            // Embedded browsers load app:// and custom schemes too, so this
            // runs before the embedded return below
            register_scheme_handlers(&self.spec);

            // Embedded mode delegates window creation to the host application which embeds CEF as a child
            // Skip browser/window creation in on_context_initialized; only register scheme handlers
            if matches!(self.spec.mode, RuntimeMode::Embedded) {
                debug!("Embedded mode; skipping window creation");
                return;
            }

            debug!("Creating main browser with URL: {}", self.spec.start_url);
            let opening = Opening::Application(self.spec.start_window.clone());
            // A CEF callback has nowhere to return the error
            if let Err(error) = open_browser_window(&self.app, &self.spec.start_url, opening) {
                error!("no window will appear: {error}");
            }
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
            let windows = self.app.registry().windows.all();

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
                let launch = SecondInstance::from_launch(command_line, current_directory);
                on_second_instance(&launch, &self.app);
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
    pub(crate) fn create(app: AppHandle, spec: RuntimeSpec) -> BrowserProcessHandler {
        let chrome_ui_client = KuroganeClient::new(app.clone(), BrowserType::ChromeUi, None);
        Self::new(app, spec, chrome_ui_client)
    }
}

/// Registers the scheme handler factories on the global request context:
/// `app://` when the application serves local assets and every scheme given
/// to [`App::register_scheme`](crate::App::register_scheme).
///
/// Nothing here keeps the factories: cef-rs adds the reference CEF adopts
/// for each one and CEF holds it until the factory is replaced or cleared.
fn register_scheme_handlers(spec: &RuntimeSpec) {
    let Some(global) = request_context_get_global_context() else {
        error!("no global request context; scheme handlers not registered");
        return;
    };

    // Register `app://` only when serving local assets; URL mode (App::url) has
    // no asset root or scheme handler.
    if let Some(root) = &spec.asset_root {
        debug!("Registering scheme handler factory for app://");
        let mut factory = crate::scheme::AppSchemeHandlerFactory::new(root.clone());
        let result = global.register_scheme_handler_factory(
            Some(&CefString::from("app")),
            Some(&CefString::from("app")),
            Some(&mut factory),
        );
        debug!("register app:// scheme handler factory result: {result}");
    }

    // User-registered custom schemes are served on any host
    for scheme in &spec.scheme_handlers {
        debug!("Registering scheme handler factory for {}://", scheme.name);
        let mut factory = crate::scheme::CustomSchemeHandlerFactory::new(scheme.handler.clone());
        let result = global.register_scheme_handler_factory(
            Some(&CefString::from(scheme.name.as_str())),
            Some(&CefString::from("")),
            Some(&mut factory),
        );
        debug!(
            "register {}:// scheme handler factory result: {result}",
            scheme.name
        );
    }
}
