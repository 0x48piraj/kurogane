use cef::{args::Args, *};
use std::sync::{Arc, Mutex};
use std::sync::atomic::AtomicBool;
use std::path::PathBuf;
use std::sync::OnceLock;

use crate::cef_app::DemoApp;
use crate::error::RuntimeError;
use crate::scheme::{CanonicalRoot, ResolveError};

static ASSET_ROOT: OnceLock<CanonicalRoot> = OnceLock::new();

/// Public entry point for launching a CEF application.
///
/// Responsible for:
/// - Initializing platform-specific CEF requirements
/// - Spawning CEF subprocesses
/// - Starting the browser process
/// - Running the CEF message loop
pub struct Runtime;

impl Runtime {
    /// Launches the CEF runtime and blocks until shutdown.
    ///
    /// start_url determines what the browser loads on startup.
    pub fn run(start_url: CefString, require_assets: bool) -> Result<(), RuntimeError> {
        if require_assets {
            Self::validate_asset_root()?;
        }

        #[cfg(target_os = "macos")]
        crate::platform::macos::init_ns_app();

        let _ = api_hash(sys::CEF_API_VERSION_LAST, 0);

        let args = Args::new();
        let window = Arc::new(Mutex::new(None));
        let window_creation_started = Arc::new(AtomicBool::new(false));

        // ONE app for ALL processes
        let mut app: App = DemoApp::new(window.clone(), start_url, window_creation_started);

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

        let exe = std::env::current_exe()
            .expect("failed to get current exe path");
        let exe_str = exe.to_string_lossy();

        // Isolate the CEF cache per executable.
        // Reusing a profile across runs can trigger session restore leading to multiple on_context_initialized invocations.
        let exe_name = exe
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "app".to_string());

        let cache_dir = std::env::temp_dir()
            .join("kurogane")
            .join(&exe_name);

        std::fs::create_dir_all(&cache_dir).ok();

        let cef_root = find_cef_root()?
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
            persist_session_cookies: 1,
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
                persist_session_cookies: 1,
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

        run_message_loop();
        shutdown();
        Ok(())
    }

    pub fn set_asset_root(path: PathBuf) -> Result<(), RuntimeError> {
        let canonical = CanonicalRoot::new(&path).map_err(|e| match e {
            ResolveError::InvalidRoot(p) => RuntimeError::InvalidAssetRoot(p),
            _ => RuntimeError::AssetRootMissing(path.clone()),
        })?;

        ASSET_ROOT
            .set(canonical)
            .map_err(|_| RuntimeError::AssetRootAlreadySet)?;

        Ok(())
    }

    pub fn asset_root() -> CanonicalRoot {
        ASSET_ROOT.get().expect("asset root not set").clone()
    }

    fn validate_asset_root() -> Result<(), RuntimeError> {
        ASSET_ROOT.get().ok_or(RuntimeError::AssetRootNotSet)?;

        Ok(())
    }
}

fn find_cef_root() -> Result<PathBuf, RuntimeError> {
    use std::env;

    // Dev environment
    if let Ok(path) = env::var("CEF_PATH") {
        let p = PathBuf::from(path);
        if p.exists() {
            return Ok(p);
        }
    }

    // Next to executable (production)
    if let Ok(exe) = env::current_exe() {
        if let Some(dir) = exe.parent() {
            #[cfg(target_os = "windows")]
            {
                // Windows bundle: CEF is flattened next to the exe.
                if dir.join("libcef.dll").exists() {
                    return Ok(dir.to_path_buf());
                }
            }

            #[cfg(target_os = "linux")]
            {
                // Linux: CEF lives in a cef/ subdirectory.
                let candidate = dir.join("cef");
                if candidate.exists() {
                    return Ok(candidate);
                }
            }
        }
    }

    Err(RuntimeError::CefNotInstalled)
}
