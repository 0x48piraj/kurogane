use cef::{args::Args, sys::cef_window_handle_t, *};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::cef_app::KuroganeApp;
use crate::client::KuroganeClient;
use crate::error::RuntimeError;
use crate::gpu::GpuMode;
use crate::chromium_flags::ChromiumFlag;
use crate::fs::CanonicalRoot;
use crate::ShutdownSignal;
use crate::browser_registry::{BrowserRegistry, BrowserId, BrowserType};
use crate::window_registry::WindowRegistry;
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

fn build_settings(layout: &RuntimeLayout, persist_session_cookies: bool, external_message_pump: bool) -> Settings {
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
            external_message_pump: external_message_pump as i32,
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
            external_message_pump: external_message_pump as i32,
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

fn install_ctrlc_handler(window_registry: Arc<Mutex<WindowRegistry>>) {
    // Prevent double-fire (dev hammers Ctrl+C twice)
    let quitting = Arc::new(AtomicBool::new(false));
    let main = window_registry.clone();

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

            let mut task = CloseWindowsTask::new(main.clone());
            post_task(ThreadId::UI, Some(&mut task));
        }
    })
    .expect("failed to install SIGINT handler");
}

wrap_task! {
    struct CloseWindowsTask {
        registry: Arc<Mutex<WindowRegistry>>,
    }

    impl Task {
        fn execute(&self) {
            let reg = self.registry.lock().unwrap();
            reg.close_all_windows();
        }
    }
}

pub(crate) struct RuntimeState {
    shutdown_signal: ShutdownSignal,
    dispatcher: Arc<IpcDispatcher>,
    registry: Arc<Mutex<BrowserRegistry>>,
    #[allow(dead_code)]
    window_registry: Arc<Mutex<WindowRegistry>>,
}

#[derive(Clone, Copy, Debug)]
pub struct BrowserBounds {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// Handle to a live initialized CEF runtime.
///
/// Enables external event-loop integration by separating runtime polling from loop ownership.
pub struct RuntimeHandle {
    state: RuntimeState,
    shutdown_called: AtomicBool,
}

impl Drop for RuntimeHandle {
    fn drop(&mut self) {
        // CEF requires shutdown on the same thread as initialize
        // Callers must not move RuntimeHandle across threads after start()
        self.shutdown();
    }
}

fn native_to_cef_window(
    handle: *mut std::ffi::c_void,
) -> cef_window_handle_t {
    #[cfg(target_os = "windows")]
    {
        use cef::sys::HWND;
        HWND(handle as *mut cef::sys::HWND__)
    }

    #[cfg(target_os = "macos")]
    {
        handle as cef_window_handle_t
    }

    #[cfg(target_os = "linux")]
    {
        handle as usize as cef_window_handle_t
    }
}

pub struct BrowserHandle {
    id: BrowserId,
    registry: Arc<Mutex<BrowserRegistry>>,
}

impl BrowserHandle {
    pub fn id(&self) -> BrowserId {
        self.id
    }

    pub fn close(&self, force: bool) {
        let browser = {
            let reg = self.registry.lock().unwrap();
            reg.get(self.id).map(|s| s.browser.clone())
        };
        if let Some(b) = browser {
            b.host().map(|h| h.close_browser(force as i32));
        }
    }

    pub fn notify_resized(&self) {
        let browser = {
            let reg = self.registry.lock().unwrap();
            reg.get(self.id).map(|s| s.browser.clone())
        };
        if let Some(b) = browser {
            b.host().map(|h| h.was_resized());
        }
    }

    pub fn notify_move_or_resize_started(&self) {
        let browser = {
            let reg = self.registry.lock().unwrap();
            reg.get(self.id).map(|s| s.browser.clone())
        };
        if let Some(b) = browser {
            b.host().map(|h| h.notify_move_or_resize_started());
        }
    }
}

impl RuntimeHandle {
    /// Advances Chromium by one iteration of its internal message loop.
    ///
    /// When using external event-loop ownership via App::start,
    /// this must be called repeatedly on the same thread.
    pub fn pump(&self) {
        do_message_loop_work();
    }

    /// Returns true when shutdown has been requested
    /// e.g. the window was closed or Ctrl+C was received.
    pub fn should_shutdown(&self) -> bool {
        self.state.shutdown_signal.is_shutdown_requested()
    }

    /// Returns the number of currently live browser instances.
    pub fn browser_count(&self) -> usize {
        self.state.registry.lock().unwrap().count()
    }

    /// Perform orderly CEF shutdown.
    ///
    /// Safe to call multiple times. Subsequent calls are no-ops.
    pub fn shutdown(&self) {
        if self.shutdown_called.swap(true, Ordering::SeqCst) {
            return;
        }
        debug!("Shutting down CEF via RuntimeHandle");
        shutdown();
        self.state.shutdown_signal.request_shutdown();
        debug!("CEF shutdown complete via RuntimeHandle");
    }

    /// Creates a Chromium browser hosted inside an existing native window.
    ///
    /// The browser is attached to parent and positioned using the provided bounds.
    ///
    /// 'parent' must be a valid platform window handle ('HWND' on Windows,
    /// 'NSView' on macOS, or the corresponding native handle on Linux)
    ///
    /// The runtime must have been started with Runtime::start_embedded,
    /// and RuntimeHandle::pump must continue to be called regularly for
    /// Chromium to process events.
    ///
    /// Returns true if browser creation succeeded.
    pub fn create_child_browser(
        &self,
        parent: *mut std::ffi::c_void,
        bounds: BrowserBounds,
        url: &str,
    ) -> Option<BrowserHandle>
    {
        self.create_child_browser_impl(parent, bounds, url, None)
    }

    /// Creates a child browser with a custom request context (separate cookie/cache partition).
    ///
    /// Same as create_child_browser but accepts RequestContextSettings to control
    /// the cache partition, cookie persistence and accept language for this browser.
    ///
    /// The runtime must have been started with Runtime::start_embedded.
    pub fn create_child_browser_with_request_context(
        &self,
        parent: *mut std::ffi::c_void,
        bounds: BrowserBounds,
        url: &str,
        rc_settings: &cef::RequestContextSettings,
    ) -> Option<BrowserHandle>
    {
        let rc = cef::request_context_create_context(Some(rc_settings), None);
        self.create_child_browser_impl(parent, bounds, url, rc)
    }
    fn create_child_browser_impl(
        &self,
        parent: *mut std::ffi::c_void,
        bounds: BrowserBounds,
        url: &str,
        request_context: Option<cef::RequestContext>,
    ) -> Option<BrowserHandle>
    {
        let info = WindowInfo {
            runtime_style: RuntimeStyle::ALLOY,
            ..WindowInfo::default()
        }.set_as_child(
            native_to_cef_window(parent),
            &Rect {
                x: bounds.x,
                y: bounds.y,
                width: bounds.width,
                height: bounds.height,
            },
        );

        let mut client = KuroganeClient::new(self.state.dispatcher.clone(), self.state.shutdown_signal.clone(), self.state.registry.clone());

        let mut rc = request_context;
        let browser = browser_host_create_browser_sync(
            Some(&info),
            Some(&mut client),
            Some(&CefString::from(url)),
            Some(&Default::default()),
            None,
            rc.as_mut(),
        )?;

        let id = self.state.registry.lock().unwrap().register_with_context(
            browser.clone(),
            BrowserType::Main,
            None,
            rc,
        );
        Some(BrowserHandle { id, registry: self.state.registry.clone() })
    }
}

/// Initializes CEF and prepares the browser process runtime.
///
/// Executes subprocess dispatch, resolves the runtime layout,
/// configures CEF settings and initializes the browser process.
///
/// Behavior differs slightly in embedded mode, where the host
/// application owns window creation and lifecycle management.
///
/// Returns the initialized runtime state on success.
fn initialize_cef(
    start_url: String,
    asset_root: Option<CanonicalRoot>,
    dispatcher: Arc<IpcDispatcher>,
    profile_id: Option<String>,
    persist_session_cookies: bool,
    gpu_mode: GpuMode,
    chromium_flags: Vec<ChromiumFlag>,
    embedded_mode: bool,
    external_message_pump: bool,
) -> Result<RuntimeState, RuntimeError> {
    #[cfg(target_os = "macos")]
    crate::platform::macos::init_ns_app();

    let _ = api_hash(sys::CEF_API_VERSION_LAST, 0);

    debug!("Runtime initializing");

    let args = Args::new();

    let shutdown_signal = ShutdownSignal::new();
    let registry = Arc::new(Mutex::new(BrowserRegistry::new(shutdown_signal.clone())));
    let window_registry = Arc::new(Mutex::new(WindowRegistry::new()));

    // ONE app for ALL processes
    let mut app: App = KuroganeApp::new(
        window_registry.clone(),
        registry.clone(),
        shutdown_signal.clone(),
        CefString::from(start_url.as_str()),
        asset_root,
        dispatcher.clone(),
        gpu_mode,
        chromium_flags,
        embedded_mode,
    );

    debug!("Executing subprocess dispatch");
    execute_subprocesses(&args, &mut app);

    let layout = resolve_layout(profile_id)?;
    let settings = build_settings(&layout, persist_session_cookies, external_message_pump);

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

    // Only install Ctrl+C handler if CEF Views owns the window (non-embedded mode)
    // In embedded mode the host application manages its own lifecycle
    if !embedded_mode {
        debug!("Installing shutdown handler");
        install_ctrlc_handler(window_registry.clone());
    }

    Ok(RuntimeState {
        shutdown_signal,
        dispatcher,
        registry,
        #[allow(dead_code)]
        window_registry,
    })
}

impl Runtime {
    /// Initialize CEF and return a RuntimeHandle without entering a message loop.
    ///
    /// The caller takes ownership of the event loop and must periodically call
    /// RuntimeHandle::pump when using Pump mode, then call
    /// RuntimeHandle::shutdown to clean up.
    pub fn start(
        start_url: String,
        asset_root: Option<CanonicalRoot>,
        dispatcher: Arc<IpcDispatcher>,
        profile_id: Option<String>,
        persist_session_cookies: bool,
        gpu_mode: GpuMode,
        chromium_flags: Vec<ChromiumFlag>,
    ) -> Result<RuntimeHandle, RuntimeError> {
        let state = initialize_cef(
            start_url,
            asset_root,
            dispatcher,
            profile_id,
            persist_session_cookies,
            gpu_mode,
            chromium_flags,
            false,
            false,
        )?;

        Ok(RuntimeHandle {
            state,
            shutdown_called: AtomicBool::new(false),
        })
    }

    /// Initialize CEF in embedded mode (no window created by CEF Views)
    pub fn start_embedded(
        start_url: String,
        asset_root: Option<CanonicalRoot>,
        dispatcher: Arc<IpcDispatcher>,
        profile_id: Option<String>,
        persist_session_cookies: bool,
        gpu_mode: GpuMode,
        chromium_flags: Vec<ChromiumFlag>,
    ) -> Result<RuntimeHandle, RuntimeError> {
        let state = initialize_cef(
            start_url,
            asset_root,
            dispatcher,
            profile_id,
            persist_session_cookies,
            gpu_mode,
            chromium_flags,
            true,
            true,
        )?;

        Ok(RuntimeHandle {
            state,
            shutdown_called: AtomicBool::new(false),
        })
    }

    /// Launches the CEF runtime and blocks until shutdown.
    ///
    /// Uses CEF's internal message loop (external_message_pump = false).
    /// Existing applications using this API continue to work unchanged.
    pub fn run(
        start_url: String,
        asset_root: Option<CanonicalRoot>,
        dispatcher: Arc<IpcDispatcher>,
        profile_id: Option<String>,
        persist_session_cookies: bool,
        gpu_mode: GpuMode,
        chromium_flags: Vec<ChromiumFlag>,
    ) -> Result<(), RuntimeError> {
        let handle = Self::start(
            start_url,
            asset_root,
            dispatcher,
            profile_id,
            persist_session_cookies,
            gpu_mode,
            chromium_flags,
        )?;

        run_message_loop();

        debug!("Message loop exited");

        debug!("Shutting down CEF");
        handle.shutdown();
        debug!("CEF shutdown complete");

        Ok(())
    }
}
