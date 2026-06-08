use cef::{args::Args, *};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::cef_app::KuroganeApp;
use crate::error::RuntimeError;
use crate::gpu::GpuMode;
use crate::chromium_flags::ChromiumFlag;
use crate::fs::CanonicalRoot;
use kurogane_layout::{detect_cef_root, validate_cef_root, profile_dir};
use crate::ipc::IpcDispatcher;
use crate::debug;

/// Public entry point for launching a CEF application.
///
/// Responsible for:
/// - Initializing platform-specific CEF requirements
/// - Spawning CEF subprocesses
/// - Starting the browser process
/// - Running the CEF message loop
pub struct Runtime;

struct RuntimeLayout {
    exe: std::path::PathBuf,
    cef_root: std::path::PathBuf,
    cache_dir: std::path::PathBuf,
    locales_dir: std::path::PathBuf,
}

fn resolve_layout(profile_id: Option<String>) -> Result<RuntimeLayout, RuntimeError> {
    debug!("Resolving runtime layout");

    // Isolate the CEF cache per executable
    // Reusing a profile across runs can trigger session restore leading to multiple on_context_initialized invocations
    let exe = std::env::current_exe()
        .expect("failed to get current exe path");

    let raw_name = profile_id.unwrap_or_else(|| "kurogane-app".to_string());

    let cache_dir = profile_dir(&raw_name, &exe);
    debug!("Cache dir: {}", cache_dir.display());

    std::fs::create_dir_all(&cache_dir).ok();

    let detected = detect_cef_root()
        .map_err(|_| RuntimeError::CefNotInstalled)?;

    validate_cef_root(&detected.root)
        .map_err(|e| {
            RuntimeError::InvalidCefInstallation(e.to_string())
        })?;

    let cef_root = detected.root
        .canonicalize()
        .map_err(|_| RuntimeError::CefNotInstalled)?;

    debug!("CEF root: {}", cef_root.display());

    let locales_dir = cef_root.join("locales");

    Ok(RuntimeLayout {
        exe,
        cef_root,
        cache_dir,
        locales_dir,
    })
}

fn build_settings(layout: &RuntimeLayout, persist_session_cookies: bool) -> Settings {
    // Use a persistent profile instead of CEF's default incognito mode
    // This enables cookies, storage APIs and service workers

    let exe_str = layout.exe.to_string_lossy();
    let cef_root_str = layout.cef_root.to_string_lossy();
    let no_sandbox: i32 = if cfg!(target_os = "linux") { 1 } else { 0 };

    #[cfg(not(target_os = "macos"))]
    {
        Settings {
            browser_subprocess_path: CefString::from(exe_str.as_ref()),
            resources_dir_path: CefString::from(cef_root_str.as_ref()),
            locales_dir_path: CefString::from(layout.locales_dir.to_string_lossy().as_ref()),
            cache_path: CefString::from(layout.cache_dir.to_string_lossy().as_ref()),
            root_cache_path: CefString::from(layout.cache_dir.to_string_lossy().as_ref()),
            persist_session_cookies: if persist_session_cookies { 1 } else { 0 },
            no_sandbox,
            ..Default::default()
        }
    }

    #[cfg(target_os = "macos")]
    {
        let mut s = Settings {
            browser_subprocess_path: CefString::from(exe_str.as_ref()),
            resources_dir_path: CefString::from(cef_root_str.as_ref()),
            locales_dir_path: CefString::from(layout.locales_dir.to_string_lossy().as_ref()),
            cache_path: CefString::from(layout.cache_dir.to_string_lossy().as_ref()),
            root_cache_path: CefString::from(layout.cache_dir.to_string_lossy().as_ref()),
            persist_session_cookies: if persist_session_cookies { 1 } else { 0 },
            no_sandbox,
            ..Default::default()
        };

        let framework = layout.cef_root.join("Chromium Embedded Framework.framework");
        s.framework_dir_path = CefString::from(framework.to_string_lossy().as_ref());

        s
    }
}

fn execute_subprocesses(args: &Args, app: &mut App) {
    debug!("Dispatching CEF process selection");

    // CEF internally determines process role here
    let exit_code = execute_process(
        Some(args.as_main_args()),
        Some(app),
        std::ptr::null_mut(),
    );

    // This was a subprocess and should exit now
    if exit_code >= 0 {
        debug!(
            "CEF subprocess completed startup; exiting with code {}",
            exit_code
        );

        std::process::exit(exit_code);
    }
    debug!("Continuing as browser process");
}

fn install_ctrlc_handler(window: Arc<Mutex<Option<Window>>>) {
    // Prevent double-fire (dev hammers Ctrl+C twice)
    let quitting = Arc::new(AtomicBool::new(false));
    let main = window.clone();

    ctrlc::set_handler({
        let quitting = quitting.clone();
        let main = main.clone();

        move || {
            debug!("SIGINT received");

            // Only act on the first signal
            if quitting.swap(true, Ordering::SeqCst) {
                debug!("Shutdown already in progress");
                return;
            }

            debug!("Scheduling window shutdown on UI thread");

            let mut task = CloseMainWindowTask::new(main.clone());
            post_task(ThreadId::UI, Some(&mut task));
        }
    })
    .expect("failed to install SIGINT handler");
}

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
        gpu_mode: GpuMode,
        chromium_flags: Vec<ChromiumFlag>,
    ) -> Result<(), RuntimeError> {
        #[cfg(target_os = "macos")]
        crate::platform::macos::init_ns_app();

        let _ = api_hash(sys::CEF_API_VERSION_LAST, 0);

        debug!("Runtime starting");

        let args = Args::new();
        let window = Arc::new(Mutex::new(None));
        let window_creation_started = Arc::new(AtomicBool::new(false));

        // ONE app for ALL processes
        let mut app: App = KuroganeApp::new(
            window.clone(),
            CefString::from(start_url.as_str()),
            asset_root,
            dispatcher,
            window_creation_started,
            gpu_mode,
            chromium_flags,
        );

        debug!("Executing subprocess dispatch");
        execute_subprocesses(&args, &mut app);

        let layout = resolve_layout(profile_id)?;
        let settings = build_settings(&layout, persist_session_cookies);

        debug!("Initializing CEF");

        if initialize(
            Some(args.as_main_args()),
            Some(&settings),
            Some(&mut app),
            std::ptr::null_mut(),
        ) != 1 {
            return Err(RuntimeError::CefInitializeFailed);
        }

        debug!("CEF initialized");

        debug!("Installing shutdown handler");
        install_ctrlc_handler(window.clone());

        debug!("Entering CEF message loop");
        run_message_loop();
        debug!("Message loop exited");

        debug!("Shutting down CEF");
        shutdown();
        debug!("CEF shutdown complete");

        Ok(())
    }
}
