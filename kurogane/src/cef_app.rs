//! Root CEF application object.

use cef::*;
use std::sync::{Arc, Mutex};
use std::sync::atomic::AtomicBool;
use std::cell::RefCell;

use crate::browser::DemoBrowserProcessHandler;
use crate::ipc::IpcRenderProcessHandler;
use crate::debug;
use crate::scheme::CanonicalRoot;
use crate::ipc::IpcDispatcher;
use crate::gpu::{GpuMode, apply_gpu_flags};
use crate::sandbox::apply_sandbox_flags;

use cef::sys::cef_scheme_options_t::*;

wrap_app! {
    pub struct DemoApp {
        window: Arc<Mutex<Option<Window>>>,
        start_url: CefString,
        asset_root: Option<CanonicalRoot>,
        dispatcher: Arc<IpcDispatcher>,
        window_creation_started: Arc<AtomicBool>,
        gpu_mode: GpuMode,
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

            #[cfg(feature = "html_canvas_compositor")]
            {
                cmd.append_switch_with_value(
                    Some(&CefString::from("enable-blink-features")),
                    Some(&CefString::from("CanvasDrawElement")),
                );
            }

            #[cfg(feature = "debug")]
            cmd.append_switch_with_value(
                Some(&CefString::from("js-flags")),
                Some(&CefString::from("--expose-gc")),
            );

            apply_sandbox_flags(cmd);
            apply_gpu_flags(cmd, self.gpu_mode);
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
                    self.asset_root.clone(),
                    self.dispatcher.clone(),
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
