use cef::{args::Args, sys::cef_window_handle_t, *};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::cef_app::KuroganeApp;
use crate::client::KuroganeClient;
use crate::error::RuntimeError;
use crate::ShutdownSignal;
use crate::browser_registry::{BrowserRegistry, BrowserId, BrowserMetadata};
use crate::window_registry::{WindowRegistry, WindowId, WindowMetadata};
use crate::window::{KuroganeBrowserViewDelegate, KuroganeWindowDelegate, WindowIdentity};
use kurogane_layout::{detect_cef_root_with_version, validate_cef_runtime, profile_dir};
use crate::ipc::IpcRouter;
use crate::spec::RuntimeSpec;
use crate::debug;

/// Public entry point for launching a CEF application.
///
/// Responsible for:
/// - Initializing platform-specific CEF requirements
/// - Spawning CEF subprocesses
/// - Starting the browser process
/// - Running the CEF message loop
pub(crate) struct RuntimeBootstrap;

struct RuntimeLayout {
    exe: std::path::PathBuf,
    cef_root: std::path::PathBuf,
    cache_dir: std::path::PathBuf,
    #[cfg(not(target_os = "macos"))]
    locales_dir: std::path::PathBuf,
}

fn resolve_layout(
    profile_id: Option<String>,
    cache_dir: Option<std::path::PathBuf>,
) -> Result<RuntimeLayout, RuntimeError> {
    debug!("Resolving runtime layout");

    // Isolate the CEF cache per executable
    // Reusing a profile across runs can trigger session restore leading to multiple on_context_initialized invocations
    let exe = std::env::current_exe().expect("failed to get current exe path");

    let raw_name = profile_id.unwrap_or_else(|| "kurogane-app".to_string());

    let cache_dir = cache_dir.unwrap_or_else(|| profile_dir(&raw_name, &exe));
    debug!("Cache dir: {}", cache_dir.display());

    std::fs::create_dir_all(&cache_dir).map_err(|e| RuntimeError::CacheUnavailable {
        path: cache_dir.clone(),
        source: e,
    })?;

    let detected = detect_cef_root_with_version(None).map_err(|_| RuntimeError::CefNotInstalled)?;

    validate_cef_runtime(&detected.root)
        .map_err(|e| RuntimeError::InvalidCefInstallation(e.to_string()))?;

    let cef_root = detected
        .root
        .canonicalize()
        .map_err(|_| RuntimeError::CefNotInstalled)?;

    debug!("CEF root: {}", cef_root.display());

    #[cfg(not(target_os = "macos"))]
    let locales_dir = cef_root.join("locales");

    Ok(RuntimeLayout {
        exe,
        cef_root,
        cache_dir,
        #[cfg(not(target_os = "macos"))]
        locales_dir,
    })
}

fn build_settings(
    layout: &RuntimeLayout,
    persist_session_cookies: bool,
    external_message_pump: bool,
) -> Settings {
    // Use a persistent profile instead of CEF's default incognito mode
    // This enables cookies, storage APIs and service workers

    let exe_str = layout.exe.to_string_lossy();
    #[cfg(not(target_os = "macos"))]
    let cef_root_str = layout.cef_root.to_string_lossy();

    // Sandbox is disabled on all platforms
    let no_sandbox: i32 = 1;

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
        // CEF resolves resources, locales and V8 snapshots from the framework bundle
        let mut s = Settings {
            browser_subprocess_path: CefString::from(exe_str.as_ref()),
            external_message_pump: external_message_pump as i32,
            cache_path: CefString::from(layout.cache_dir.to_string_lossy().as_ref()),
            root_cache_path: CefString::from(layout.cache_dir.to_string_lossy().as_ref()),
            persist_session_cookies: if persist_session_cookies { 1 } else { 0 },
            no_sandbox,
            ..Default::default()
        };

        let framework = layout
            .cef_root
            .join("Chromium Embedded Framework.framework");
        s.framework_dir_path = CefString::from(framework.to_string_lossy().as_ref());

        s
    }
}

fn execute_subprocesses(args: &Args, app: &mut App) {
    debug!("Dispatching CEF process selection");

    // CEF internally determines process role here
    let exit_code = execute_process(Some(args.as_main_args()), Some(app), std::ptr::null_mut());

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

fn install_ctrlc_handler(
    browser_registry: Arc<Mutex<BrowserRegistry>>,
    window_registry: Arc<Mutex<WindowRegistry>>,
) {
    // Prevent double-fire (dev hammers Ctrl+C twice)
    let quitting = Arc::new(AtomicBool::new(false));

    ctrlc::set_handler({
        let quitting = quitting.clone();
        let browser_registry = browser_registry.clone();
        let window_registry = window_registry.clone();

        move || {
            debug!("SIGINT received");

            // Only act on the first signal
            if quitting.swap(true, Ordering::SeqCst) {
                debug!("Shutdown already in progress");
                return;
            }

            debug!("Scheduling browser shutdown on UI thread");

            let mut task = CloseAllTask::new(browser_registry.clone(), window_registry.clone());
            post_task(ThreadId::UI, Some(&mut task));
        }
    })
    .expect("failed to install SIGINT handler");
}

pub(crate) fn close_all_browsers_and_windows(
    browser_registry: &Arc<Mutex<BrowserRegistry>>,
    window_registry: &Arc<Mutex<WindowRegistry>>,
) {
    // Close all browsers first; in Views mode this cascades to close their parent windows
    // Embedded mode has no Views windows
    let browsers: Vec<Browser> = {
        let reg = browser_registry.lock().unwrap();
        reg.iter().map(|(_, s)| s.browser.clone()).collect()
    };
    for browser in browsers {
        if let Some(host) = browser.host() {
            debug!("closing browser cef_id={}", browser.identifier());
            host.close_browser(false as i32);
        }
    }

    // Close any remaining Views windows not closed by the browser cascade
    let wreg = window_registry.lock().unwrap();
    wreg.close_all_windows();
}

wrap_task! {
    struct CloseAllTask {
        browser_registry: Arc<Mutex<BrowserRegistry>>,
        window_registry: Arc<Mutex<WindowRegistry>>,
    }

    impl Task {
        fn execute(&self) {
            close_all_browsers_and_windows(&self.browser_registry, &self.window_registry);
        }
    }
}

/// Live shared runtime services.
///
/// Handlers and delegates should depend on RuntimeServices
/// instead of receiving individual registries and dispatchers separately.
pub(crate) struct RuntimeServices {
    pub shutdown_signal: ShutdownSignal,
    pub router: Arc<IpcRouter>,
    pub browser_registry: Arc<Mutex<BrowserRegistry>>,
    pub window_registry: Arc<Mutex<WindowRegistry>>,
}

pub(crate) struct RuntimeState {
    pub services: Arc<RuntimeServices>,
    pub ui_thread_id: std::thread::ThreadId,
}

#[derive(Clone, Copy, Debug)]
pub struct BrowserBounds {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// Initial visibility state for a newly created window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WindowState {
    /// Show the window normally.
    #[default]
    Normal,

    /// Create the window minimized.
    Minimized,

    /// Create the window maximized.
    Maximized,

    /// Create the window hidden.
    Hidden,
}

impl From<WindowState> for cef::ShowState {
    fn from(state: WindowState) -> Self {
        match state {
            WindowState::Normal => cef::ShowState::NORMAL,
            WindowState::Minimized => cef::ShowState::MINIMIZED,
            WindowState::Maximized => cef::ShowState::MAXIMIZED,
            WindowState::Hidden => cef::ShowState::HIDDEN,
        }
    }
}

impl From<cef::ShowState> for WindowState {
    fn from(state: cef::ShowState) -> Self {
        match state {
            cef::ShowState::NORMAL => Self::Normal,
            cef::ShowState::MINIMIZED => Self::Minimized,
            cef::ShowState::MAXIMIZED => Self::Maximized,
            cef::ShowState::HIDDEN => Self::Hidden,
            other => {
                debug_assert!(false, "unsupported cef::ShowState: {:?}", other);
                Self::Normal
            }
        }
    }
}

/// Options for creating a new top-level browser window.
#[derive(Debug, Clone)]
pub struct WindowOptions {
    /// Initial URL to load.
    pub url: String,

    /// Initial window position and size.
    pub bounds: BrowserBounds,

    /// Initial visibility state of the window.
    pub show_state: WindowState,
}

/// Shared inner state for AppHandle
struct AppHandleInner {
    services: Arc<RuntimeServices>,
    ui_thread_id: std::thread::ThreadId,
    cef_shutdown_called: AtomicBool,
}

/// Shared lifecycle handle for a running Kurogane application.
///
/// AppHandle can be used from any thread to query state,
/// broadcast events, or signal shutdown.
///
/// Obtain one via AppInstance::handle().
pub struct AppHandle {
    inner: Arc<AppHandleInner>,
}

impl Clone for AppHandle {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

// SAFETY:
//
// AppHandleInner is shared across threads via Arc. The only CEF object it
// indirectly reaches (cef::Frame, stored in EventSubscription) is accessed
// exclusively via Frame::send_process_message, which CEF documents as
// safe to call from any thread. No other Frame methods are invoked from
// non-UI threads through AppHandle.
unsafe impl Send for AppHandle {}
unsafe impl Sync for AppHandle {}

impl AppHandle {
    fn services(&self) -> &RuntimeServices {
        &self.inner.services
    }

    /// Signals the CEF message loop to exit.
    ///
    /// Safe to call from any thread. CEF posts the quit internally to the UI
    /// thread. The actual cef::shutdown() call happens on the UI thread in
    /// AppInstance::run or AppInstance::shutdown after the loop exits.
    pub fn shutdown(&self) {
        self.services().shutdown_signal.request_shutdown();
        debug!("AppHandle::shutdown: quitting message loop");
        quit_message_loop();
    }

    /// Returns true when shutdown has been requested
    /// (e.g. via shutdown(Self::shutdown), window close, or Ctrl+C).
    pub fn should_shutdown(&self) -> bool {
        self.services().shutdown_signal.is_shutdown_requested()
    }

    /// Broadcast an event to all renderers subscribed to event.
    ///
    /// The event is delivered asynchronously to every active subscription for the
    /// given event name. This method is thread-safe and returns immediately after
    /// queuing the event for delivery.
    pub fn broadcast(&self, event: &str, data: &[u8]) {
        self.services().router.event.broadcast(event, data);
    }

    /// Broadcast a JSON-serializable event to all renderers subscribed to event.
    ///
    /// The value is serialized to JSON and sent as a string payload.
    /// This is the preferred way to emit structured events.
    pub fn broadcast_json<T: serde::Serialize>(&self, event: &str, value: &T) {
        if let Ok(json) = serde_json::to_string(value) {
            self.broadcast(event, json.as_bytes());
        }
    }

    /// Number of currently live browser instances.
    pub fn browser_count(&self) -> usize {
        self.services().browser_registry.lock().unwrap().count()
    }

    /// Number of currently open windows.
    pub fn window_count(&self) -> usize {
        self.services().window_registry.lock().unwrap().count()
    }

    /// IDs of all open windows.
    pub fn window_ids(&self) -> Vec<WindowId> {
        let reg = self.services().window_registry.lock().unwrap();
        reg.iter().map(|(id, _)| *id).collect()
    }

    /// Close all open windows.
    pub fn close_all_windows(&self) {
        let reg = self.services().window_registry.lock().unwrap();
        reg.close_all_windows();
    }

    /// Close all live browser instances.
    pub fn close_all_browsers(&self, force: bool) {
        let browsers: Vec<Browser> = {
            let reg = self.services().browser_registry.lock().unwrap();
            reg.iter().map(|(_, s)| s.browser.clone()).collect()
        };
        for browser in browsers {
            debug!("calling close_browser on cef_id={}", browser.identifier());
            if let Some(host) = browser.host() {
                host.close_browser(force as i32);
            }
        }
    }

    /// Look up the window that hosts a given browser.
    pub fn find_window_by_browser(&self, browser_id: BrowserId) -> Option<WindowId> {
        self.services()
            .window_registry
            .lock()
            .unwrap()
            .window_id_for_browser(browser_id)
    }

    /// Metadata for all live browsers.
    pub fn browsers(&self) -> Vec<(BrowserId, BrowserMetadata)> {
        let reg = self.services().browser_registry.lock().unwrap();
        reg.iter()
            .map(|(id, s)| (*id, s.metadata.clone()))
            .collect()
    }

    /// Metadata for all open windows.
    pub fn windows(&self) -> Vec<(WindowId, WindowMetadata)> {
        let reg = self.services().window_registry.lock().unwrap();
        reg.iter()
            .map(|(id, s)| (*id, s.metadata.clone()))
            .collect()
    }

    /// Parent of a given browser.
    pub fn browser_parent(&self, id: BrowserId) -> Option<BrowserId> {
        self.services()
            .browser_registry
            .lock()
            .unwrap()
            .browser_parent(id)
    }

    /// Opener of a given browser.
    pub fn browser_opener(&self, id: BrowserId) -> Option<BrowserId> {
        self.services()
            .browser_registry
            .lock()
            .unwrap()
            .browser_opener(id)
    }

    /// All children of the given parent browser.
    pub fn children_of(&self, id: BrowserId) -> Vec<BrowserId> {
        self.services()
            .browser_registry
            .lock()
            .unwrap()
            .children_of(id)
    }

    /// Browser hosted in the given window.
    pub fn browser_for_window(&self, id: WindowId) -> Option<BrowserId> {
        self.services()
            .window_registry
            .lock()
            .unwrap()
            .browser_for_window(id)
    }

    /// Creates a BrowserHandle for a registered browser, if it exists.
    ///
    /// Returns None if no browser with the given BrowserId is registered.
    pub fn get_browser_handle(&self, id: BrowserId) -> Option<BrowserHandle> {
        let reg = self.services().browser_registry.lock().unwrap();
        if reg.get(id).is_some() {
            Some(BrowserHandle {
                id,
                browser_registry: self.services().browser_registry.clone(),
                ui_thread_id: self.inner.ui_thread_id,
            })
        } else {
            None
        }
    }
}

impl Drop for AppInstance {
    fn drop(&mut self) {
        // CEF requires shutdown to occur on the same thread that performed initialization
        // The runtime must remain on its originating UI thread for its entire lifetime
        // Do NOT move the runtime to another thread after startup
        self.shutdown();
    }
}

fn native_to_cef_window(handle: *mut std::ffi::c_void) -> cef_window_handle_t {
    let result;

    #[cfg(target_os = "windows")]
    {
        use cef::sys::HWND;
        result = HWND(handle as *mut cef::sys::HWND__);
    }

    #[cfg(target_os = "macos")]
    {
        result = handle as cef_window_handle_t;
    }

    #[cfg(target_os = "linux")]
    {
        result = handle as usize as cef_window_handle_t;
    }

    result
}

pub struct BrowserHandle {
    id: BrowserId,
    browser_registry: Arc<Mutex<BrowserRegistry>>,
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
            let reg = self.browser_registry.lock().unwrap();
            reg.get(self.id).map(|s| s.browser.clone())
        };
        if let Some(b) = browser {
            debug!(
                "close browser cef_id={} is_loading={}",
                b.identifier(),
                b.is_loading()
            );
            if let Some(h) = b.host() {
                h.close_browser(force as i32);
            }
        }
    }

    pub fn notify_resized(&self) {
        self.assert_ui_thread();
        let browser = {
            let reg = self.browser_registry.lock().unwrap();
            reg.get(self.id).map(|s| s.browser.clone())
        };
        if let Some(b) = browser
            && let Some(h) = b.host()
        {
            h.was_resized();
        }
    }

    pub fn notify_move_or_resize_started(&self) {
        self.assert_ui_thread();
        let browser = {
            let reg = self.browser_registry.lock().unwrap();
            reg.get(self.id).map(|s| s.browser.clone())
        };
        if let Some(b) = browser
            && let Some(h) = b.host()
        {
            h.notify_move_or_resize_started();
        }
    }

    /// Navigate the main frame to the given URL.
    pub fn navigate(&self, url: &str) {
        self.assert_ui_thread();

        let browser = {
            let reg = self.browser_registry.lock().unwrap();
            reg.get(self.id).map(|s| s.browser.clone())
        };

        if let Some(b) = browser
            && let Some(frame) = b.main_frame()
        {
            let url = CefString::from(url);
            frame.load_url(Some(&url));
        }
    }

    /// Reload the current page.
    pub fn reload(&self) {
        self.assert_ui_thread();
        let browser = {
            let reg = self.browser_registry.lock().unwrap();
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
            let reg = self.browser_registry.lock().unwrap();
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
            let reg = self.browser_registry.lock().unwrap();
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
            let reg = self.browser_registry.lock().unwrap();
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
            let reg = self.browser_registry.lock().unwrap();
            reg.get(self.id).map(|s| s.browser.clone())
        };
        browser.map(|b| b.can_go_back() != 0).unwrap_or(false)
    }

    /// Returns true if the browser can go forward.
    pub fn can_go_forward(&self) -> bool {
        self.assert_ui_thread();
        let browser = {
            let reg = self.browser_registry.lock().unwrap();
            reg.get(self.id).map(|s| s.browser.clone())
        };
        browser.map(|b| b.can_go_forward() != 0).unwrap_or(false)
    }

    /// Returns true if the browser is currently loading.
    pub fn is_loading(&self) -> bool {
        self.assert_ui_thread();
        let browser = {
            let reg = self.browser_registry.lock().unwrap();
            reg.get(self.id).map(|s| s.browser.clone())
        };
        browser.map(|b| b.is_loading() != 0).unwrap_or(false)
    }

    /// Returns the current URL of the main frame.
    pub fn url(&self) -> String {
        self.assert_ui_thread();
        let browser = {
            let reg = self.browser_registry.lock().unwrap();
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
            let reg = self.browser_registry.lock().unwrap();
            reg.get(self.id).map(|s| s.browser.clone())
        };

        if let Some(b) = browser
            && let Some(frame) = b.main_frame()
        {
            let code = CefString::from(code);
            let script_url = CefString::from(script_url);

            frame.execute_java_script(Some(&code), Some(&script_url), start_line);
        }
    }

    /// Open DevTools for this browser.
    pub fn show_devtools(&self) {
        self.assert_ui_thread();
        let browser = {
            let reg = self.browser_registry.lock().unwrap();
            reg.get(self.id).map(|s| s.browser.clone())
        };
        if let Some(b) = browser
            && let Some(h) = b.host()
        {
            h.show_dev_tools(None, None, None, None);
        }
    }

    /// Close DevTools if open.
    pub fn close_devtools(&self) {
        self.assert_ui_thread();
        let browser = {
            let reg = self.browser_registry.lock().unwrap();
            reg.get(self.id).map(|s| s.browser.clone())
        };
        if let Some(b) = browser
            && let Some(h) = b.host()
        {
            h.close_dev_tools();
        }
    }

    /// Returns true if DevTools is currently open for this browser.
    pub fn has_devtools(&self) -> bool {
        self.assert_ui_thread();
        let browser = {
            let reg = self.browser_registry.lock().unwrap();
            reg.get(self.id).map(|s| s.browser.clone())
        };
        browser
            .and_then(|b| b.host())
            .map(|h| h.has_dev_tools() != 0)
            .unwrap_or(false)
    }
}

// AppInstance: UI-thread lifecycle owner
pub struct AppInstance {
    handle: AppHandle,
}

impl AppInstance {
    /// Returns the shared handle, usable from any thread.
    pub fn handle(&self) -> &AppHandle {
        &self.handle
    }

    /// Advances Chromium by one iteration of its internal message loop.
    ///
    /// When using external event-loop ownership via App::start,
    /// this must be called repeatedly on the thread that initialized CEF.
    ///
    /// Note: Kurogane currently assumes pump calls are non-reentrant and
    /// originate from a single UI thread.
    pub fn pump(&self) {
        do_message_loop_work();
    }

    /// Returns true when shutdown has been requested
    /// e.g. the window was closed or Ctrl+C was received.
    pub fn should_shutdown(&self) -> bool {
        self.handle.should_shutdown()
    }

    /// Broadcast an event to all renderer processes.
    pub fn broadcast(&self, event: &str, data: &[u8]) {
        self.handle.broadcast(event, data);
    }

    /// Broadcast a JSON-serializable event to all renderer processes.
    pub fn broadcast_json<T: serde::Serialize>(&self, event: &str, value: &T) {
        self.handle.broadcast_json(event, value);
    }

    /// Creates a new top-level window with an embedded browser.
    pub fn create_window(&self, options: WindowOptions) -> Result<WindowId, RuntimeError> {
        let is_closing = Arc::new(AtomicBool::new(false));

        let mut client =
            KuroganeClient::new(self.handle.inner.services.clone(), is_closing.clone());

        let mut bv_delegate = KuroganeBrowserViewDelegate::new(
            self.handle.inner.services.browser_registry.clone(),
            self.handle.inner.services.window_registry.clone(),
        );

        let url = CefString::from(options.url.as_str());

        let browser_view = browser_view_create(
            Some(&mut client),
            Some(&url),
            Some(&Default::default()),
            None,
            None,
            Some(&mut bv_delegate),
        )
        .ok_or(RuntimeError::BrowserCreationFailed)?;

        let window_id = {
            let mut reg = self.handle.inner.services.window_registry.lock().unwrap();
            reg.allocate_id()
        };

        let mut delegate = KuroganeWindowDelegate::new(
            window_id,
            browser_view,
            self.handle.inner.services.window_registry.clone(),
            Rect {
                x: options.bounds.x,
                y: options.bounds.y,
                width: options.bounds.width,
                height: options.bounds.height,
            },
            options.show_state.into(),
            is_closing,
            WindowIdentity::default(),
        );

        window_create_top_level(Some(&mut delegate)).ok_or(RuntimeError::WindowCreationFailed)?;

        Ok(window_id)
    }

    /// Blocks until the browser associated with the given window is registered,
    /// or until the timeout expires.
    ///
    /// Pumps the message loop internally while waiting.
    pub fn wait_for_browser(
        &self,
        window_id: WindowId,
        timeout: std::time::Duration,
    ) -> Option<BrowserHandle> {
        let start = std::time::Instant::now();
        loop {
            if let Some(browser_id) = self.handle.browser_for_window(window_id) {
                return self.handle.get_browser_handle(browser_id);
            }
            if start.elapsed() >= timeout {
                return None;
            }
            self.pump();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    /// Takes ownership and blocks on the CEF message loop.
    ///
    /// The loop runs until AppHandle::shutdown is called (from any thread)
    /// or all browser windows close. After the loop exits, cef::shutdown()
    /// is called on the current (UI) thread.
    pub fn run(self) -> Result<(), RuntimeError> {
        run_message_loop();

        debug!("Message loop exited");

        if self
            .handle
            .inner
            .cef_shutdown_called
            .swap(true, Ordering::SeqCst)
        {
            debug!("CEF shutdown already performed");
            return Ok(());
        }

        debug!("Shutting down CEF");
        shutdown();
        debug!("CEF shutdown complete");

        Ok(())
    }

    /// Perform orderly CEF shutdown.
    ///
    /// Sets the shutdown signal and calls cef::shutdown() on the UI thread.
    /// Safe to call multiple times. Subsequent calls are no-ops.
    pub fn shutdown(&self) {
        if self
            .handle
            .inner
            .cef_shutdown_called
            .swap(true, Ordering::SeqCst)
        {
            return;
        }

        debug!("Shutting down Kurogane runtime");
        shutdown();
        self.handle
            .inner
            .services
            .shutdown_signal
            .request_shutdown();
        debug!("Kurogane runtime shutdown complete");
    }

    /// Creates a Chromium browser hosted inside an existing native window.
    ///
    /// The browser is attached to parent and positioned using the provided bounds.
    ///
    /// 'parent' must be a valid platform window handle ('HWND' on Windows,
    /// 'NSView' on macOS, or the corresponding native handle on Linux)
    ///
    /// The runtime must have been started with Runtime::start_embedded,
    /// and Runtime::pump must continue to be called regularly for
    /// Chromium to process events.
    ///
    /// Returns true if browser creation succeeded.
    pub fn create_child_browser(
        &self,
        parent: *mut std::ffi::c_void,
        bounds: BrowserBounds,
        url: &str,
    ) -> Option<BrowserHandle> {
        self.create_child_browser_impl(parent, bounds, url, None)
    }

    /// Creates a child browser with a custom request context (separate cookie/cache partition).
    ///
    /// Same as create_child_browser but accepts RequestContextSettings to control
    /// the cache partition, cookie persistence and accept language for this browser.
    ///
    /// The runtime must have been started with RuntimeBootstrap::start_embedded.
    pub fn create_child_browser_with_request_context(
        &self,
        parent: *mut std::ffi::c_void,
        bounds: BrowserBounds,
        url: &str,
        rc_settings: &cef::RequestContextSettings,
    ) -> Option<BrowserHandle> {
        let rc = cef::request_context_create_context(Some(rc_settings), None);
        self.create_child_browser_impl(parent, bounds, url, rc)
    }

    fn create_child_browser_impl(
        &self,
        parent: *mut std::ffi::c_void,
        bounds: BrowserBounds,
        url: &str,
        request_context: Option<cef::RequestContext>,
    ) -> Option<BrowserHandle> {
        let info = WindowInfo {
            runtime_style: RuntimeStyle::ALLOY,
            ..WindowInfo::default()
        }
        .set_as_child(
            native_to_cef_window(parent),
            &Rect {
                x: bounds.x,
                y: bounds.y,
                width: bounds.width,
                height: bounds.height,
            },
        );

        let is_closing = Arc::new(AtomicBool::new(false));
        let mut client = KuroganeClient::new(self.handle.inner.services.clone(), is_closing);

        let mut rc = request_context;
        let browser = browser_host_create_browser_sync(
            Some(&info),
            Some(&mut client),
            Some(&CefString::from(url)),
            Some(&Default::default()),
            None,
            rc.as_mut(),
        )?;

        debug!("create_child_browser_impl cef_id={}", browser.identifier());

        let id = {
            let reg = self.handle.inner.services.browser_registry.lock().unwrap();

            reg.find_id_by_cef_id(browser.identifier())
                .expect("browser should have been registered by on_after_created")
        };

        Some(BrowserHandle {
            id,
            browser_registry: self.handle.inner.services.browser_registry.clone(),
            ui_thread_id: self.handle.inner.ui_thread_id,
        })
    }

    /// Number of live browser instances.
    pub fn browser_count(&self) -> usize {
        self.handle.browser_count()
    }

    /// Number of open windows.
    pub fn window_count(&self) -> usize {
        self.handle.window_count()
    }

    /// IDs of all open windows.
    pub fn window_ids(&self) -> Vec<WindowId> {
        self.handle.window_ids()
    }

    /// Close all open windows.
    pub fn close_all_windows(&self) {
        self.handle.close_all_windows()
    }

    /// Close all live browser instances.
    pub fn close_all_browsers(&self, force: bool) {
        self.handle.close_all_browsers(force)
    }

    /// Look up the window that hosts a given browser.
    pub fn find_window_by_browser(&self, browser_id: BrowserId) -> Option<WindowId> {
        self.handle.find_window_by_browser(browser_id)
    }

    /// Metadata for all live browsers.
    pub fn browsers(&self) -> Vec<(BrowserId, BrowserMetadata)> {
        self.handle.browsers()
    }

    /// Metadata for all open windows.
    pub fn windows(&self) -> Vec<(WindowId, WindowMetadata)> {
        self.handle.windows()
    }

    /// Parent of a given browser.
    pub fn browser_parent(&self, id: BrowserId) -> Option<BrowserId> {
        self.handle.browser_parent(id)
    }

    /// Returns the opener BrowserId for a given browser, if any.
    pub fn browser_opener(&self, id: BrowserId) -> Option<BrowserId> {
        self.handle.browser_opener(id)
    }

    /// Returns all browsers whose parent is the given BrowserId.
    pub fn children_of(&self, id: BrowserId) -> Vec<BrowserId> {
        self.handle.children_of(id)
    }

    /// Returns the BrowserId hosted in the given window, if any.
    pub fn browser_for_window(&self, id: WindowId) -> Option<BrowserId> {
        self.handle.browser_for_window(id)
    }

    /// Create a BrowserHandle for a registered browser id.
    pub fn get_browser_handle(&self, id: BrowserId) -> Option<BrowserHandle> {
        self.handle.get_browser_handle(id)
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
    spec: RuntimeSpec,
    router: Arc<IpcRouter>,
    embedded_mode: bool,
) -> Result<RuntimeState, RuntimeError> {
    #[cfg(target_os = "macos")]
    crate::platform::macos::init_ns_app()?;

    let _ = api_hash(sys::CEF_API_VERSION_LAST, 0);

    debug!("Runtime initializing");

    let args = Args::new();

    let shutdown_signal = ShutdownSignal::new();
    let browser_registry = Arc::new(Mutex::new(BrowserRegistry::new(shutdown_signal.clone())));
    let window_registry = Arc::new(Mutex::new(WindowRegistry::new()));

    let services = Arc::new(RuntimeServices {
        shutdown_signal,
        router,
        browser_registry,
        window_registry,
    });

    #[cfg(target_os = "macos")]
    crate::platform::macos::set_services(services.clone());

    // ONE app for ALL processes
    let mut app: App = KuroganeApp::new(services.clone(), spec.clone());

    debug!("Executing subprocess dispatch");
    execute_subprocesses(&args, &mut app);

    let layout = resolve_layout(spec.profile_id, spec.cache_dir)?;
    let external_message_pump = spec.scheduler.is_some();
    let settings = build_settings(&layout, spec.persist_session_cookies, external_message_pump);

    debug!("Initializing CEF");

    if initialize(
        Some(args.as_main_args()),
        Some(&settings),
        Some(&mut app),
        std::ptr::null_mut(),
    ) != 1
    {
        return Err(RuntimeError::CefInitializeFailed);
    }

    debug!("CEF initialized");

    #[cfg(target_os = "macos")]
    crate::platform::macos::setup_app_delegate();

    // Only install Ctrl+C handler if CEF Views owns the window (non-embedded mode)
    // In embedded mode the host application manages its own lifecycle
    if !embedded_mode {
        debug!("Installing shutdown handler");
        install_ctrlc_handler(
            services.browser_registry.clone(),
            services.window_registry.clone(),
        );
    }

    Ok(RuntimeState {
        services,
        ui_thread_id: std::thread::current().id(),
    })
}

impl RuntimeBootstrap {
    /// Initialize CEF and return an AppInstance without entering a message loop.
    pub(crate) fn start(
        spec: RuntimeSpec,
        router: Arc<IpcRouter>,
    ) -> Result<AppInstance, RuntimeError> {
        let state = initialize_cef(spec, router, false)?;
        let handle = AppHandle {
            inner: Arc::new(AppHandleInner {
                services: state.services,
                ui_thread_id: state.ui_thread_id,
                cef_shutdown_called: AtomicBool::new(false),
            }),
        };
        Ok(AppInstance { handle })
    }

    /// Initialize CEF in embedded mode (no window created by CEF Views)
    pub(crate) fn start_embedded(
        spec: RuntimeSpec,
        router: Arc<IpcRouter>,
    ) -> Result<AppInstance, RuntimeError> {
        let state = initialize_cef(spec, router, true)?;
        let handle = AppHandle {
            inner: Arc::new(AppHandleInner {
                services: state.services,
                ui_thread_id: state.ui_thread_id,
                cef_shutdown_called: AtomicBool::new(false),
            }),
        };
        Ok(AppInstance { handle })
    }
}
