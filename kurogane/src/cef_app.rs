//! Root CEF application object.

use cef::*;
use std::sync::Arc;
use std::cell::RefCell;

use crate::browser::KuroganeBrowserProcessHandler;
use crate::ipc::IpcRenderProcessHandler;
use crate::runtime::RuntimeServices;
use crate::spec::RuntimeSpec;
use crate::debug;
use crate::chromium_flags::ChromiumFlags;
use crate::gpu::apply_gpu_flags;
use crate::sandbox::apply_sandbox_flags;

use cef::sys::cef_scheme_options_t::*;

wrap_app! {
    pub struct KuroganeApp {
        services: Arc<RuntimeServices>,
        spec: RuntimeSpec,
    }

    impl App {
        fn on_before_command_line_processing(
            &self,
            process_type: Option<&CefString>,
            command_line: Option<&mut CommandLine>,
        ) {
            let Some(cmd) = command_line else { return };

            // Dispatch to lifecycle delegates first
            for delegate in &self.spec.delegates {
                delegate.on_before_command_line_processing(cmd);
            }

            // Startup policy is currently only applied to the main browser process
            // Chromium propagates the relevant switches to child processes
            if process_type.is_some() {
                return;
            }

            let mut flags = ChromiumFlags::default();

            #[cfg(feature = "debug")]
            {
                flags.set_with_value("js-flags", "--expose-gc");
            }

            apply_sandbox_flags(&mut flags);
            apply_gpu_flags(&mut flags, self.spec.gpu_mode);

            // Apply user overrides
            flags.extend_user_flags(&self.spec.chromium_flags);

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
                    self.services.clone(),
                    self.spec.clone(),
                    RefCell::new(None),
                    RefCell::new(None),
                )
            )
        }

        fn render_process_handler(&self) -> Option<RenderProcessHandler> {
            Some(IpcRenderProcessHandler::new(self.spec.renderer_delegates.clone()))
        }
    }
}
