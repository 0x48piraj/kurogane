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
use crate::browser_registry::{BrowserRegistry, BrowserId, BrowserType, BrowserMetadata};
use crate::window_registry::{WindowRegistry, WindowId, WindowMetadata};
use crate::window::{KuroganeWindowDelegate, KuroganeBrowserViewDelegate};
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
    window_registry: Arc<Mutex<WindowRegistry>>,
    ui_thread_id: std::thread::ThreadId,
}

#[derive(Clone, Copy, Debug)]
pub struct BrowserBounds {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// Options for creating a new top-level window via RuntimeHandle::create_window.
#[derive(Debug, Clone)]
pub struct WindowOptions {
    pub url: String,
    pub bounds: BrowserBounds,
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
    ui_thread_id: std::thread::ThreadId,
}

impl BrowserHandle {
    fn assert_ui_thread(&self) {
        debug_assert_eq!(
            std::thread::current().id(),
            self.ui_thread_id,
            "BrowserHandle methods must be called from the UI thread where the runtime was initialized"
        );
    }

    pub fn id(&self) -> BrowserId {
        self.assert_ui_thread();
        self.id
    }

    pub fn close(&self, force: bool) {
        self.assert_ui_thread();
        let browser = {
            let reg = self.registry.lock().unwrap();
            reg.get(self.id).map(|s| s.browser.clone())
        };
        if let Some(b) = browser {
            b.host().map(|h| h.close_browser(force as i32));
        }
    }

    pub fn notify_resized(&self) {
        self.assert_ui_thread();
        let browser = {
            let reg = self.registry.lock().unwrap();
            reg.get(self.id).map(|s| s.browser.clone())
        };
        if let Some(b) = browser {
            b.host().map(|h| h.was_resized());
        }
    }

    pub fn notify_move_or_resize_started(&self) {
        self.assert_ui_thread();
        let browser = {
            let reg = self.registry.lock().unwrap();
            reg.get(self.id).map(|s| s.browser.clone())
        };
        if let Some(b) = browser {
            b.host().map(|h| h.notify_move_or_resize_started());
        }
    }

    /// Navigate the main frame to the given URL.
    pub fn navigate(&self, url: &str) {
        self.assert_ui_thread();

        let browser = {
            let reg = self.registry.lock().unwrap();
            reg.get(self.id).map(|s| s.browser.clone())
        };

        if let Some(b) = browser {
            if let Some(frame) = b.main_frame() {
                let url = CefString::from(url);
                frame.load_url(Some(&url));
            }
        }
    }

    /// Reload the current page.
    pub fn reload(&self) {
        self.assert_ui_thread();
        let browser = {
            let reg = self.registry.lock().unwrap();
            reg.get(self.id).map(|s| s.browser.clone())
        };
        if let Some(b) = browser {
            b.reload();
        }
    }

    /// Reload the current page, ignoring cached content.
    pub fn reload_ignore_cache(&self) {
        self.assert_ui_thread();
        let browser = {
            let reg = self.registry.lock().unwrap();
            reg.get(self.id).map(|s| s.browser.clone())
        };
        if let Some(b) = browser {
            b.reload_ignore_cache();
        }
    }

    /// Navigate back in history, if possible.
    pub fn go_back(&self) {
        self.assert_ui_thread();
        let browser = {
            let reg = self.registry.lock().unwrap();
            reg.get(self.id).map(|s| s.browser.clone())
        };
        if let Some(b) = browser {
            b.go_back();
        }
    }

    /// Navigate forward in history, if possible.
    pub fn go_forward(&self) {
        self.assert_ui_thread();
        let browser = {
            let reg = self.registry.lock().unwrap();
            reg.get(self.id).map(|s| s.browser.clone())
        };
        if let Some(b) = browser {
            b.go_forward();
        }
    }

    /// Returns true if the browser can go back.
    pub fn can_go_back(&self) -> bool {
        self.assert_ui_thread();
        let browser = {
            let reg = self.registry.lock().unwrap();
            reg.get(self.id).map(|s| s.browser.clone())
        };
        browser.map(|b| b.can_go_back() != 0).unwrap_or(false)
    }

    /// Returns true if the browser can go forward.
    pub fn can_go_forward(&self) -> bool {
        self.assert_ui_thread();
        let browser = {
            let reg = self.registry.lock().unwrap();
            reg.get(self.id).map(|s| s.browser.clone())
        };
        browser.map(|b| b.can_go_forward() != 0).unwrap_or(false)
    }

    /// Returns true if the browser is currently loading.
    pub fn is_loading(&self) -> bool {
        self.assert_ui_thread();
        let browser = {
            let reg = self.registry.lock().unwrap();
            reg.get(self.id).map(|s| s.browser.clone())
        };
        browser.map(|b| b.is_loading() != 0).unwrap_or(false)
    }

    /// Returns the current URL of the main frame.
    pub fn url(&self) -> String {
        self.assert_ui_thread();
        let browser = {
            let reg = self.registry.lock().unwrap();
            reg.get(self.id).map(|s| s.browser.clone())
        };
        browser
            .and_then(|b| b.main_frame())
            .map(|f| {
                let c: CefString = (&f.url()).into();
                c.to_string()
            })
            .unwrap_or_default()
    }

    /// Execute JavaScript in the main frame.
    pub fn execute_javascript(&self, code: &str, script_url: &str, start_line: i32) {
        self.assert_ui_thread();

        let browser = {
            let reg = self.registry.lock().unwrap();
            reg.get(self.id).map(|s| s.browser.clone())
        };

        if let Some(b) = browser {
            if let Some(frame) = b.main_frame() {
                let code = CefString::from(code);
                let script_url = CefString::from(script_url);

                frame.execute_java_script(
                    Some(&code),
                    Some(&script_url),
                    start_line,
                );
            }
        }
    }

    /// Open DevTools for this browser.
    pub fn show_devtools(&self) {
        self.assert_ui_thread();
        let browser = {
            let reg = self.registry.lock().unwrap();
            reg.get(self.id).map(|s| s.browser.clone())
        };
        if let Some(b) = browser {
            b.host().map(|h| {
                h.show_dev_tools(None, None, None, None);
            });
        }
    }

    /// Close DevTools if open.
    pub fn close_devtools(&self) {
        self.assert_ui_thread();
        let browser = {
            let reg = self.registry.lock().unwrap();
            reg.get(self.id).map(|s| s.browser.clone())
        };
        if let Some(b) = browser {
            b.host().map(|h| h.close_dev_tools());
        }
    }

    /// Returns true if DevTools is currently open for this browser.
    pub fn has_devtools(&self) -> bool {
        self.assert_ui_thread();
        let browser = {
            let reg = self.registry.lock().unwrap();
            reg.get(self.id).map(|s| s.browser.clone())
        };
        browser
            .and_then(|b| b.host())
            .map(|h| h.has_dev_tools() != 0)
            .unwrap_or(false)
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

    /// Returns the number of currently open windows.
    pub fn window_count(&self) -> usize {
        self.state.window_registry.lock().unwrap().count()
    }

    /// Returns the IDs of all open windows.
    pub fn window_ids(&self) -> Vec<WindowId> {
        let reg = self.state.window_registry.lock().unwrap();
        reg.iter().map(|(id, _)| *id).collect()
    }

    /// Close all open windows.
    pub fn close_all_windows(&self) {
        let reg = self.state.window_registry.lock().unwrap();
        reg.close_all_windows();
    }

    /// Look up the window that hosts a given browser, if any.
    pub fn find_window_by_browser(&self, browser_id: BrowserId) -> Option<WindowId> {
        self.state.window_registry.lock().unwrap()
            .window_id_for_browser(browser_id)
    }

    /// Returns metadata for all live browsers.
    pub fn browsers(&self) -> Vec<(BrowserId, BrowserMetadata)> {
        let reg = self.state.registry.lock().unwrap();
        reg.iter().map(|(id, s)| (*id, s.metadata.clone())).collect()
    }

    /// Returns metadata for all open windows.
    pub fn windows(&self) -> Vec<(WindowId, WindowMetadata)> {
        let reg = self.state.window_registry.lock().unwrap();
        reg.iter().map(|(id, s)| (*id, s.metadata.clone())).collect()
    }

    /// Returns the parent BrowserId for a given browser, if any.
    pub fn browser_parent(&self, id: BrowserId) -> Option<BrowserId> {
        self.state.registry.lock().unwrap().browser_parent(id)
    }

    /// Returns the opener BrowserId for a given browser, if any.
    pub fn browser_opener(&self, id: BrowserId) -> Option<BrowserId> {
        self.state.registry.lock().unwrap().browser_opener(id)
    }

    /// Returns all browsers whose parent is the given BrowserId.
    pub fn children_of(&self, id: BrowserId) -> Vec<BrowserId> {
        self.state.registry.lock().unwrap().children_of(id)
    }

    /// Returns the BrowserId hosted in the given window, if any.
    pub fn browser_for_window(&self, id: WindowId) -> Option<BrowserId> {
        self.state.window_registry.lock().unwrap().browser_for_window(id)
    }

    /// Creates a new top-level window with an embedded browser.
    ///
    /// The window is created using CEF Views (window_create_top_level + browser_view_create).
    /// The browser is created asynchronously; use RuntimeHandle::wait_for_browser or poll
    /// RuntimeHandle::browser_for_window to obtain the BrowserHandle once ready.
    ///
    /// Must be called on the UI thread.
    pub fn create_window(&self, options: WindowOptions) -> Result<WindowId, RuntimeError> {
        let mut client = KuroganeClient::new(
            self.state.dispatcher.clone(),
            self.state.shutdown_signal.clone(),
            self.state.registry.clone(),
            self.state.window_registry.clone(),
        );

        let mut bv_delegate = KuroganeBrowserViewDelegate::new(
            self.state.registry.clone(),
            self.state.window_registry.clone(),
        );

        let url = CefString::from(options.url.as_str());

        let browser_view = browser_view_create(
            Some(&mut client),
            Some(&url),
            Some(&Default::default()),
            None,
            None,
            Some(&mut bv_delegate),
        ).ok_or(RuntimeError::BrowserCreationFailed)?;

        let window_id = {
            let mut reg = self.state.window_registry.lock().unwrap();
            reg.allocate_id()
        };

        let mut delegate = KuroganeWindowDelegate::new(
            window_id,
            browser_view,
            self.state.window_registry.clone(),
        );

        window_create_top_level(Some(&mut delegate))
            .ok_or(RuntimeError::WindowCreationFailed)?;

        Ok(window_id)
    }

    /// Creates a BrowserHandle for a registered browser, if it exists.
    ///
    /// Returns None if no browser with the given BrowserId is registered.
    pub fn get_browser_handle(&self, id: BrowserId) -> Option<BrowserHandle> {
        let reg = self.state.registry.lock().unwrap();
        if reg.get(id).is_some() {
            Some(BrowserHandle {
                id,
                registry: self.state.registry.clone(),
                ui_thread_id: self.state.ui_thread_id,
            })
        } else {
            None
        }
    }

    /// Blocks until the browser associated with the given window is registered,
    /// or until the timeout expires.
    ///
    /// Pumps the message loop internally while waiting.
    pub fn wait_for_browser(&self, window_id: WindowId, timeout: std::time::Duration) -> Option<BrowserHandle> {
        let start = std::time::Instant::now();
        loop {
            if let Some(browser_id) = self.browser_for_window(window_id) {
                return self.get_browser_handle(browser_id);
            }
            if start.elapsed() >= timeout {
                return None;
            }
            self.pump();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
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

        let mut client = KuroganeClient::new(self.state.dispatcher.clone(), self.state.shutdown_signal.clone(), self.state.registry.clone(), self.state.window_registry.clone());

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
        Some(BrowserHandle { id, registry: self.state.registry.clone(), ui_thread_id: self.state.ui_thread_id })
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
        window_registry,
        ui_thread_id: std::thread::current().id(),
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
