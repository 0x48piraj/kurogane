use cef::{args::Args, *};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use std::path::{Path, PathBuf};
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
        start_url: CefString,
        require_assets: bool,
        profile_id: Option<String>,
        persist_session_cookies: bool,
    ) -> Result<(), RuntimeError> {
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
        let exe_canonical = exe
            .canonicalize()
            .unwrap_or(exe.clone());

        let exe_hash = fnv1a_64(&exe_canonical);


        let raw_name = profile_id
            .unwrap_or_else(|| "kurogane-app".to_string());

        let profile_name = sanitize_name(&raw_name);

        let profile_dir_name = format!("{}-{}", profile_name, exe_hash);

        let base_dir = dirs::cache_dir()
            .unwrap_or_else(|| std::env::temp_dir());

        let cache_dir = base_dir
            .join("kurogane")
            .join("profiles")
            .join(profile_dir_name);

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

/// Computes a deterministic FNV-1a 64-bit hash of a filesystem path.
/// Intended for identity stability, not cryptographic use.
fn fnv1a_64(path: &Path) -> String {
    let mut hash: u64 = 14695981039346656037;

    for byte in path.to_string_lossy().as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(1099511628211);
    }

    format!("{:016x}", hash)
}

/// Sanitizes a user-provided name into a filesystem-safe identifier.
/// Returns "default" when the input cannot be reduced to a valid name.
fn sanitize_name(name: &str) -> String {
    // Windows reserved names
    const WINDOWS_RESERVED: &[&str] = &[
        "CON", "PRN", "AUX", "NUL",
        "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9",
        "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];

    // Replace forbidden/control chars with _
    let replaced = name.chars().map(|c| match c {
        '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0' => '_',
        _ if c.is_control() => '_',
        _ => c,
    }).collect::<String>();

    // Collapse consecutive _ for aesthetics
    let mut sanitized = replaced
        .chars()
        .fold(String::new(), |mut acc, c| {
            if c == '_' && acc.ends_with('_') {
                acc
            } else {
                acc.push(c);
                acc
            }
        });

    // Trim Windows-invalid endings
    sanitized = sanitized.trim_end_matches(['.', ' ']).to_string();

    // Trim leading dots
    sanitized = sanitized.trim_start_matches('.').to_string();

    let stem = sanitized.split('.').next().unwrap();

    if WINDOWS_RESERVED.iter().any(|&r| r.eq_ignore_ascii_case(stem)) {
        sanitized = format!("_{sanitized}");
    }

    // Length limit
    const MAX_LEN: usize = 64;
    if sanitized.len() > MAX_LEN {
        sanitized.truncate(MAX_LEN);
    }

    // Fallback if empty, or _ for aesthetics, again
    if sanitized.is_empty() || sanitized.chars().all(|c| c == '_') {
        return "kurogane-app".to_string();
    }

    sanitized
}
