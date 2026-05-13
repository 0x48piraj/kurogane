use cef::{args::Args, *};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::cef_app::DemoApp;
use crate::error::RuntimeError;
use crate::scheme::CanonicalRoot;
use kurogane_layout::{detect_cef_root, validate_cef_root, profile_dir};
use crate::ipc::IpcDispatcher;

/// Public entry point for launching a CEF application.
///
/// Responsible for:
/// - Initializing platform-specific CEF requirements
/// - Spawning CEF subprocesses
/// - Starting the browser process
/// - Running the CEF message loop
pub struct Runtime;

wrap_task! {
    struct CloseMainWindowTask {
        window: Arc<Mutex<Option<Window>>>,
    }

    impl Task {
        fn execute(&self) {
            if let Some(window) = self.window.lock().unwrap().as_ref() {
                let w = window.clone();
                w.close();
            } else {
                quit_message_loop();
            }
        }
    }
}

impl Runtime {
    /// Launches the CEF runtime and blocks until shutdown.
    ///
    /// start_url determines what the browser loads on startup.
    pub fn run(
        start_url: String,
        asset_root: Option<CanonicalRoot>,
        dispatcher: Arc<IpcDispatcher>,
        profile_id: Option<String>,
        persist_session_cookies: bool,
    ) -> Result<(), RuntimeError> {
        #[cfg(target_os = "macos")]
        crate::platform::macos::init_ns_app();

        let _ = api_hash(sys::CEF_API_VERSION_LAST, 0);

        let args = Args::new();
        let window = Arc::new(Mutex::new(None));
        let window_creation_started = Arc::new(AtomicBool::new(false));

        // ONE app for ALL processes
        let mut app: App = DemoApp::new(
            window.clone(),
            CefString::from(start_url.as_str()),
            asset_root,
            dispatcher,
            window_creation_started,
        );

        // CEF internally determines process role here
        let exit_code = execute_process(
            Some(args.as_main_args()),
            Some(&mut app),
            std::ptr::null_mut(),
        );

        // This was a subprocess and should exit now
        if exit_code >= 0 {
            std::process::exit(exit_code);
        }

        // Isolate the CEF cache per executable.
        // Reusing a profile across runs can trigger session restore leading to multiple on_context_initialized invocations.
        let exe = std::env::current_exe()
            .expect("failed to get current exe path");
        let exe_str = exe.to_string_lossy();

        let raw_name = profile_id
            .unwrap_or_else(|| "kurogane-app".to_string());

        let cache_dir = profile_dir(
            &raw_name,
            &exe,
        );

        std::fs::create_dir_all(&cache_dir).ok();

        let detected = detect_cef_root()
            .map_err(|_| RuntimeError::CefNotInstalled)?;

        validate_cef_root(&detected.root)
            .map_err(|e| {
                RuntimeError::InvalidCefInstallation(
                    e.to_string()
                )
            })?;

        let cef_root = detected.root
            .canonicalize()
            .map_err(|_| RuntimeError::CefNotInstalled)?;

        let cef_root_str = cef_root.to_string_lossy();

        let no_sandbox: i32 = if cfg!(target_os = "linux") { 1 } else { 0 };

        let locales_dir = cef_root.join("locales");

        // Use a persistent profile instead of CEF's default incognito mode.
        // This enables cookies, storage APIs and service workers.

        #[cfg(not(target_os = "macos"))]
        let settings = Settings {
            browser_subprocess_path: CefString::from(exe_str.as_ref()),
            resources_dir_path: CefString::from(cef_root_str.as_ref()),
            locales_dir_path: CefString::from(locales_dir.to_string_lossy().as_ref()),
            cache_path: CefString::from(cache_dir.to_string_lossy().as_ref()),
            root_cache_path: CefString::from(cache_dir.to_string_lossy().as_ref()),
            persist_session_cookies: if persist_session_cookies { 1 } else { 0 },
            no_sandbox,

            ..Default::default()
        };

        #[cfg(target_os = "macos")]
        let settings = {
            let mut s = Settings {
                browser_subprocess_path: CefString::from(exe_str.as_ref()),
                resources_dir_path: CefString::from(cef_root_str.as_ref()),
                locales_dir_path: CefString::from(locales_dir.to_string_lossy().as_ref()),
                cache_path: CefString::from(cache_dir.to_string_lossy().as_ref()),
                root_cache_path: CefString::from(cache_dir.to_string_lossy().as_ref()),
                persist_session_cookies: if persist_session_cookies { 1 } else { 0 },
                no_sandbox,

                ..Default::default()
            };

            let framework = cef_root.join("Chromium Embedded Framework.framework");
            s.framework_dir_path = CefString::from(framework.to_string_lossy().as_ref());

            s
        };

        if initialize(
            Some(args.as_main_args()),
            Some(&settings),
            Some(&mut app),
            std::ptr::null_mut(),
        ) != 1 {
            return Err(RuntimeError::CefInitializeFailed);
        }

        // Prevent double-fire (dev hammers Ctrl+C twice)
        let quitting = Arc::new(AtomicBool::new(false));
        let main = window.clone();

        ctrlc::set_handler({
            let quitting = quitting.clone();
            let main = main.clone();

            move || {
                // Only act on the first signal
                if quitting.swap(true, Ordering::SeqCst) {
                    return;
                }

                let mut task = CloseMainWindowTask::new(main.clone());
                post_task(ThreadId::UI, Some(&mut task));
            }
        })
        .expect("failed to install SIGINT handler");

        run_message_loop();
        shutdown();
        Ok(())
    }
}
