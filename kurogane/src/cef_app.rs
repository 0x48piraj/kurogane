//! Root CEF application object.

use cef::*;
use std::sync::{Arc, Mutex};
use std::cell::RefCell;

use crate::browser::KuroganeBrowserProcessHandler;
use crate::ipc::IpcRenderProcessHandler;
use crate::debug;
use crate::fs::CanonicalRoot;
use crate::ipc::IpcDispatcher;
use crate::chromium_flags::{ChromiumFlag, ChromiumFlags};
use crate::gpu::{GpuMode, apply_gpu_flags};
use crate::sandbox::apply_sandbox_flags;
use crate::ShutdownSignal;
use crate::browser_registry::BrowserRegistry;
use crate::window_registry::WindowRegistry;
use crate::app::PumpScheduler;

use cef::sys::cef_scheme_options_t::*;

wrap_app! {
    pub struct KuroganeApp {
        window_registry: Arc<Mutex<WindowRegistry>>,
        registry: Arc<Mutex<BrowserRegistry>>,
        shutdown_signal: ShutdownSignal,
        start_url: CefString,
        asset_root: Option<CanonicalRoot>,
        dispatcher: Arc<IpcDispatcher>,
        gpu_mode: GpuMode,
        chromium_flags: Vec<ChromiumFlag>,
        embedded_mode: bool,
        scheduler: Option<PumpScheduler>,
    }

    impl App {
        fn on_before_command_line_processing(
            &self,
            process_type: Option<&CefString>,
            command_line: Option<&mut CommandLine>,
        ) {
            // Startup policy is currently only applied to the main browser process
            // Chromium propagates the relevant switches to child processes
            if process_type.is_some() {
                return;
            }

            let Some(cmd) = command_line else { return };

            let mut flags = ChromiumFlags::default();

            #[cfg(feature = "debug")]
            {
                flags.set_with_value("js-flags", "--expose-gc");
            }

            apply_sandbox_flags(&mut flags);
            apply_gpu_flags(&mut flags, self.gpu_mode);

            // Apply user overrides
            flags.extend_user_flags(&self.chromium_flags);

            debug!("Chromium startup flags:\n{}", flags);

            flags.apply(cmd);
        }

        fn on_register_custom_schemes(
            &self,
            registrar: Option<&mut SchemeRegistrar>,
        ) {
            debug!("on_register_custom_schemes called!");

            let registrar = registrar.unwrap();

            let flags =
                CEF_SCHEME_OPTION_STANDARD as i32 |
                CEF_SCHEME_OPTION_SECURE as i32 |
                CEF_SCHEME_OPTION_CORS_ENABLED as i32 |
                CEF_SCHEME_OPTION_FETCH_ENABLED as i32;

            let result = registrar.add_custom_scheme(
                Some(&CefString::from("app")),
                flags,
            );

            debug!("Registered 'app://' scheme with flags {} result: {}", flags, result);
        }

        fn browser_process_handler(&self) -> Option<BrowserProcessHandler> {
            Some(
                KuroganeBrowserProcessHandler::new(
                    self.window_registry.clone(),
                    self.registry.clone(),
                    self.shutdown_signal.clone(),
                    self.start_url.clone(),
                    self.asset_root.clone(),
                    self.dispatcher.clone(),
                    RefCell::new(None),
                    self.embedded_mode,
                    self.scheduler.clone(),
                )
            )
        }

        fn render_process_handler(&self) -> Option<RenderProcessHandler> {
            Some(IpcRenderProcessHandler::new())
        }
    }
}
