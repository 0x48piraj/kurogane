//! Root CEF application object.

use cef::*;
use std::sync::{Arc, Mutex};
use std::sync::atomic::AtomicBool;
use std::cell::RefCell;

use crate::browser::DemoBrowserProcessHandler;
use crate::ipc::IpcRenderProcessHandler;
use crate::debug;

use cef::sys::cef_scheme_options_t::*;

wrap_app! {
    pub struct DemoApp {
        window: Arc<Mutex<Option<Window>>>,
        start_url: CefString,
        window_creation_started: Arc<AtomicBool>,
    }

    impl App {
        fn on_before_command_line_processing(
            &self,
            process_type: Option<&CefString>,
            command_line: Option<&mut CommandLine>,
        ) {
            if process_type.is_some() {
                // Only configure the main browser process
                return;
            }

            let Some(cmd) = command_line else { return };

            #[cfg(target_os = "windows")]
            {
                // Sandbox disable
                cmd.append_switch(Some(&CefString::from("no-sandbox")));
                cmd.append_switch(Some(&CefString::from("disable-gpu-sandbox")));

                // Run GPU work inside the browser process rather than in a child.
                //
                // On real hardware this has no downside: hardware acceleration still
                // works, the GPU code just runs in-process instead of a child.
                cmd.append_switch(Some(&CefString::from("in-process-gpu")));
            }

            #[cfg(target_os = "linux")]
            {
                cmd.append_switch(Some(&CefString::from("no-sandbox")));
                cmd.append_switch(Some(&CefString::from("disable-setuid-sandbox")));
                cmd.append_switch(Some(&CefString::from("in-process-gpu")));
                cmd.append_switch_with_value(
                    Some(&CefString::from("ozone-platform-hint")),
                    Some(&CefString::from("auto")),
                );
            }

            #[cfg(target_os = "macos")]
            {
                cmd.append_switch(Some(&CefString::from("enable-metal")));
            }
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
                DemoBrowserProcessHandler::new(
                    self.window.clone(),
                    self.start_url.clone(),
                    RefCell::new(None),
                    self.window_creation_started.clone(),
                )
            )
        }

        fn render_process_handler(&self) -> Option<RenderProcessHandler> {
            Some(IpcRenderProcessHandler::new())
        }
    }
}
