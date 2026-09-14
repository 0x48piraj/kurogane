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
use crate::credentials::apply_credential_flags;
use crate::gpu::apply_gpu_flags;
use crate::sandbox::apply_sandbox_flags;

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

            apply_sandbox_flags(&mut flags, self.spec.sandbox_mode);
            apply_gpu_flags(&mut flags, self.spec.gpu_mode);
            apply_credential_flags(&mut flags, self.spec.credential_storage);

            // Apply user overrides
            flags.extend_user_flags(&self.spec.chromium_flags);

            for name in crate::sandbox::sandbox_overrides(&flags, self.spec.sandbox_mode) {
                eprintln!("kurogane: sandbox_mode(Chromium) is weakened by user flag --{name}");
            }

            debug!("Chromium startup flags:\n{}", flags);

            flags.apply(cmd);
        }

        fn on_register_custom_schemes(
            &self,
            registrar: Option<&mut SchemeRegistrar>,
        ) {
            debug!("on_register_custom_schemes called!");

            let registrar = registrar.unwrap();

            let flags = crate::scheme::custom_scheme_flags();

            let result = registrar.add_custom_scheme(
                Some(&CefString::from("app")),
                flags,
            );

            debug!("Registered 'app://' scheme with flags {} result: {}", flags, result);

            for scheme in &self.spec.scheme_handlers {
                let result = registrar.add_custom_scheme(
                    Some(&CefString::from(scheme.name.as_str())),
                    flags,
                );
                debug!(
                    "Registered '{}://' scheme with flags {} result: {}",
                    scheme.name, flags, result
                );
            }
        }

        fn browser_process_handler(&self) -> Option<BrowserProcessHandler> {
            Some(
                KuroganeBrowserProcessHandler::new(
                    self.services.clone(),
                    self.spec.clone(),
                    RefCell::new(Vec::new()),
                    RefCell::new(None),
                )
            )
        }

        fn render_process_handler(&self) -> Option<RenderProcessHandler> {
            Some(IpcRenderProcessHandler::new(self.spec.renderer_delegates.clone()))
        }
    }
}
