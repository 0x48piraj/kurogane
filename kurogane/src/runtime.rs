use cef::{args::Args, sys::cef_window_handle_t, *};
use std::marker::PhantomData;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::cef_app::KuroganeApp;
use crate::client::KuroganeClient;
use crate::error::RuntimeError;
use crate::ShutdownSignal;
use crate::browser_registry::{BrowserRegistry, BrowserId, BrowserMetadata, BrowserType};
use crate::window_registry::{WindowRegistry, WindowId, WindowMetadata};
use crate::window::{KuroganeWindowDelegate, KuroganeBrowserViewDelegate};
use kurogane_layout::{DetectError, detect_cef_root_with_version, validate_cef_runtime, profile_dir};
use crate::ipc::IpcRouter;
use crate::spec::{RuntimeMode, RuntimeSpec, SandboxMode};
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
    cef_root: std::path::PathBuf,
    cache_dir: std::path::PathBuf,
    /// The executable CEF starts helper processes from, when it is named.
    subprocess: Option<std::path::PathBuf>,
}

fn resolve_layout(profile_id: Option<String>) -> Result<RuntimeLayout, RuntimeError> {
    debug!("Resolving runtime layout");

    let exe = std::env::current_exe().map_err(RuntimeError::ExecutableUnavailable)?;

    let cache_dir = profile_dir(&profile_name(profile_id, &exe));
    debug!("Cache dir: {}", cache_dir.display());

    std::fs::create_dir_all(&cache_dir).map_err(|e| RuntimeError::CacheUnavailable {
        path: cache_dir.clone(),
        source: e,
    })?;

    let detected = detect_cef_root_with_version(None).map_err(cef_not_found)?;

    validate_cef_runtime(&detected.root)
        .map_err(|e| RuntimeError::InvalidCefInstallation(e.to_string()))?;

    let cef_root = detected.root.canonicalize().map_err(|e| {
        RuntimeError::InvalidCefInstallation(format!("{}: {e}", detected.root.display()))
    })?;

    debug!("CEF root: {}", cef_root.display());

    Ok(RuntimeLayout {
        cef_root,
        cache_dir,
        subprocess: subprocess_path(&exe),
    })
}

/// Names the application's profile.
///
/// The name is the identity given to [`App::profile_id`](crate::App::profile_id),
/// or else the executable's, which `kurogane run`, bundles and the sandbox
/// bootstrap all share and which survives the application moving or being
/// updated. CEF runs one instance per profile, so the name also decides which
/// launches hand over to a running instance. Debug builds keep a profile of
/// their own, so a development run never hands its launch to an installed copy.
fn profile_name(profile_id: Option<String>, exe: &std::path::Path) -> String {
    let id = profile_id
        .or_else(|| {
            exe.file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| "kurogane-app".to_owned());

    if cfg!(debug_assertions) {
        format!("{id}-dev")
    } else {
        id
    }
}

/// Maps a failure to find the Chromium runtime onto what the user can act on.
pub(crate) fn cef_not_found(error: DetectError) -> RuntimeError {
    match error {
        DetectError::CurrentExe(source) => RuntimeError::ExecutableUnavailable(source),
        // Not found and whatever a newer layout crate adds, leaves no runtime
        _ => RuntimeError::CefNotInstalled,
    }
}

/// Returns the executable CEF should start helper processes from.
///
/// Windows names none. CEF reads any value there as "the helpers are a
/// separate executable", which its Windows sandbox does not support, and
/// turns the sandbox off without saying so. Helpers relaunch this executable
/// either way, which is what the setting would have named.
#[cfg(target_os = "windows")]
fn subprocess_path(_exe: &std::path::Path) -> Option<std::path::PathBuf> {
    None
}

/// Returns the executable CEF should start helper processes from.
#[cfg(target_os = "linux")]
fn subprocess_path(exe: &std::path::Path) -> Option<std::path::PathBuf> {
    Some(exe.to_path_buf())
}

/// Returns the executable CEF should start helper processes from: the
/// bundle's helper app, or this executable when running unbundled.
#[cfg(target_os = "macos")]
fn subprocess_path(exe: &std::path::Path) -> Option<std::path::PathBuf> {
    let helper = kurogane_layout::bundled_helper_path_for(exe);

    Some(helper.unwrap_or_else(|| exe.to_path_buf()))
}

fn build_settings(
    layout: &RuntimeLayout,
    persist_session_cookies: bool,
    external_message_pump: bool,
    sandbox: SandboxMode,
) -> Settings {
    // Use a persistent profile instead of CEF's default incognito mode
    // This enables cookies, storage APIs and service workers
    let mut settings = Settings {
        external_message_pump: external_message_pump.into(),
        cache_path: cef_path(&layout.cache_dir),
        root_cache_path: cef_path(&layout.cache_dir),
        persist_session_cookies: persist_session_cookies.into(),
        no_sandbox: crate::sandbox::cef_no_sandbox(sandbox),
        ..Default::default()
    };

    if let Some(subprocess) = &layout.subprocess {
        debug!("Browser subprocess path: {}", subprocess.display());
        settings.browser_subprocess_path = cef_path(subprocess);
    }

    // CEF resolves resources, locales and V8 snapshots from the framework bundle
    #[cfg(target_os = "macos")]
    {
        let framework = layout
            .cef_root
            .join("Chromium Embedded Framework.framework");
        settings.framework_dir_path = cef_path(&framework);
    }

    #[cfg(not(target_os = "macos"))]
    {
        settings.resources_dir_path = cef_path(&layout.cef_root);
        settings.locales_dir_path = cef_path(&layout.cef_root.join("locales"));
    }

    settings
}

fn cef_path(path: &std::path::Path) -> CefString {
    CefString::from(path.to_string_lossy().as_ref())
}

/// Returns whether this process is Chromium's browser process rather than
/// one of its helper processes (renderer, GPU, utility) which run this same
/// binary again with a `--type=` argument.
///
/// Code before [`App::run`](crate::App::run) runs in every process. Use this
/// to guard one-time side effects:
///
/// ```no_run
/// if kurogane::is_browser_process() {
///     // create files, print, open sockets; once, not once per helper
/// }
/// kurogane::App::new("content").run_or_exit();
/// ```
pub fn is_browser_process() -> bool {
    browser_process_from_args(std::env::args_os())
}

/// Returns whether the arguments identify a subprocess.
fn browser_process_from_args<I, S>(args: I) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    !args
        .into_iter()
        .any(|arg| arg.as_ref().to_string_lossy().starts_with("--type="))
}

fn execute_subprocesses(args: &Args, app: &mut App, sandbox_info: *mut u8) {
    debug!("Dispatching CEF process selection");

    // CEF internally determines process role here
    let exit_code = execute_process(Some(args.as_main_args()), Some(app), sandbox_info);

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

fn install_ctrlc_handler(services: Arc<RuntimeServices>) {
    // Prevent double-fire (dev hammers Ctrl+C twice)
    let quitting = Arc::new(AtomicBool::new(false));

    let installed = ctrlc::set_handler({
        let quitting = quitting.clone();

        move || {
            debug!("SIGINT received");

            // Only act on the first signal
            if quitting.swap(true, Ordering::SeqCst) {
                debug!("Shutdown already in progress");
                return;
            }

            debug!("Scheduling browser shutdown on UI thread");

            // Unload handlers still run, as for a window closed by hand
            let mut task = CloseAllTask::new(services.clone(), false);
            post_task(ThreadId::UI, Some(&mut task));
        }
    });

    // A host that installed its own handler keeps it; the app still closes
    // normally, only not on Ctrl+C
    if let Err(err) = installed {
        eprintln!("kurogane: Ctrl+C will not close the app: {err}");
    }
}

/// Closes all browsers, then any remaining Views windows. UI thread only.
///
/// Shutdown completes from the last browser's `OnBeforeClose` callback. If
/// there are no browsers or windows, it completes immediately. A window whose
/// browser is still being created remains open for this purpose.
///
/// `force` uses `CloseBrowser(true)`. It also closes unlinked Views windows
/// directly; linked windows close with their browser.
pub(crate) fn close_all(services: &RuntimeServices, force: bool) {
    let no_browsers = services.browser_registry.lock().unwrap().is_empty();
    let no_windows = services.window_registry.lock().unwrap().count() == 0;
    if no_browsers && no_windows {
        services.shutdown_signal.request_shutdown();
        quit_message_loop();
        return;
    }

    // Close all browsers first; in Views mode this cascades to close their parent windows
    // Embedded mode has no Views windows
    close_browsers(&services.browser_registry, force);

    // In a forced close, linked windows close with their browser. Close only
    // unlinked windows directly.
    let windows = {
        let reg = services.window_registry.lock().unwrap();
        if force { reg.unlinked() } else { reg.all() }
    };
    close_windows(windows);
}

/// Closes the given Views windows. UI thread only.
///
/// Copy the windows before calling `Window::close` so the registry is not
/// held while the window delegate runs.
fn close_windows(windows: Vec<Window>) {
    for window in windows {
        window.close();
    }
}

/// Asks every live browser to close.
///
/// CEF allows `BrowserHost` calls from any thread of the browser process.
fn close_browsers(browser_registry: &Mutex<BrowserRegistry>, force: bool) {
    let browsers: Vec<Browser> = {
        let reg = browser_registry.lock().unwrap();
        reg.iter().map(|(_, s)| s.browser.clone()).collect()
    };

    for browser in browsers {
        if let Some(host) = browser.host() {
            debug!("closing browser cef_id={}", browser.identifier());
            host.close_browser(force as i32);
        }
    }
}

/// Returns whether this is the thread CEF's UI work must run on.
fn on_ui_thread() -> bool {
    currently_on(ThreadId::UI) != 0
}

wrap_task! {
    struct CloseAllTask {
        services: Arc<RuntimeServices>,
        force: bool,
    }

    impl Task {
        fn execute(&self) {
            close_all(&self.services, self.force);
        }
    }
}

wrap_task! {
    struct CloseWindowsTask {
        window_registry: Arc<Mutex<WindowRegistry>>,
    }

    impl Task {
        fn execute(&self) {
            let windows = self.window_registry.lock().unwrap().all();
            close_windows(windows);
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

// SAFETY: the CEF objects an `AppHandle` reaches (browsers and windows in the
// registries, frames in event subscriptions) are reference counted by CEF
// with atomic counts, so they may be cloned and dropped on any thread. The
// calls made on them from other threads are ones CEF allows from any thread
// of the browser process (`Browser`, `BrowserHost` and `Frame` methods).
// Views windows and the message loop are UI-thread only, so `close_all_windows`
// and `shutdown` post to the UI thread, and `BrowserHandle` asserts it.
unsafe impl Send for AppHandle {}

// SAFETY: as for `Send`. Shared access to the registries goes through their
// mutexes and to the shutdown state through atomics and no method hands out
// a reference to a CEF object.
unsafe impl Sync for AppHandle {}

impl AppHandle {
    fn services(&self) -> &RuntimeServices {
        &self.inner.services
    }

    /// Ends the application by closing all browsers.
    ///
    /// The application ends after the last browser closes. Calling this from
    /// another thread posts the close to the UI thread. The call does not wait
    /// for the browsers to close.
    ///
    /// With [`App::start_embedded`](crate::App::start_embedded), browser closing
    /// also waits for the host window to close.
    pub fn shutdown(&self) {
        debug!("AppHandle::shutdown: closing every browser");

        if on_ui_thread() {
            close_all(self.services(), true);
        } else {
            let mut task = CloseAllTask::new(self.inner.services.clone(), true);
            post_task(ThreadId::UI, Some(&mut task));
        }
    }

    /// Returns whether the application has ended.
    ///
    /// Becomes true after the last browser closes, after
    /// [`AppHandle::shutdown`], or when the application receives a quit request.
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
    ///
    /// Safe to call from any thread: CEF's windows close only on the UI
    /// thread, so a call from elsewhere is posted there.
    pub fn close_all_windows(&self) {
        let registry = &self.services().window_registry;

        if on_ui_thread() {
            let windows = registry.lock().unwrap().all();
            close_windows(windows);
        } else {
            post_task(
                ThreadId::UI,
                Some(&mut CloseWindowsTask::new(registry.clone())),
            );
        }
    }

    /// Close all live browser instances.
    pub fn close_all_browsers(&self, force: bool) {
        close_browsers(&self.services().browser_registry, force);
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

#[cfg(target_os = "windows")]
fn native_to_cef_window(handle: *mut std::ffi::c_void) -> cef_window_handle_t {
    cef::sys::HWND(handle.cast())
}

#[cfg(target_os = "macos")]
fn native_to_cef_window(handle: *mut std::ffi::c_void) -> cef_window_handle_t {
    handle as cef_window_handle_t
}

#[cfg(target_os = "linux")]
fn native_to_cef_window(handle: *mut std::ffi::c_void) -> cef_window_handle_t {
    handle as usize as cef_window_handle_t
}

pub struct BrowserHandle {
    id: BrowserId,
    browser_registry: Arc<Mutex<BrowserRegistry>>,
    ui_thread_id: std::thread::ThreadId,
}

impl BrowserHandle {
    /// Ensures CEF UI-thread affinity for this handle.
    #[track_caller]
    fn assert_ui_thread(&self) {
        assert_eq!(
            std::thread::current().id(),
            self.ui_thread_id,
            "BrowserHandle methods must be called from the UI thread where the runtime was initialized"
        );
    }

    /// Returns the browser this handle names, while it is open.
    #[track_caller]
    fn browser(&self) -> Option<Browser> {
        self.assert_ui_thread();
        let reg = self.browser_registry.lock().unwrap();
        reg.get(self.id).map(|s| s.browser.clone())
    }

    /// Returns the host of the browser this handle names, while it is open.
    #[track_caller]
    fn host(&self) -> Option<BrowserHost> {
        self.browser()?.host()
    }

    pub fn id(&self) -> BrowserId {
        self.assert_ui_thread();
        self.id
    }

    pub fn close(&self, force: bool) {
        if let Some(b) = self.browser() {
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
        if let Some(h) = self.host() {
            h.was_resized();
        }
    }

    pub fn notify_move_or_resize_started(&self) {
        if let Some(h) = self.host() {
            h.notify_move_or_resize_started();
        }
    }

    /// Navigate the main frame to the given URL.
    pub fn navigate(&self, url: &str) {
        if let Some(frame) = self.browser().and_then(|b| b.main_frame()) {
            let url = CefString::from(url);
            frame.load_url(Some(&url));
        }
    }

    /// Reload the current page.
    pub fn reload(&self) {
        if let Some(b) = self.browser() {
            b.reload();
        }
    }

    /// Reload the current page, ignoring cached content.
    pub fn reload_ignore_cache(&self) {
        if let Some(b) = self.browser() {
            b.reload_ignore_cache();
        }
    }

    /// Navigate back in history, if possible.
    pub fn go_back(&self) {
        if let Some(b) = self.browser() {
            b.go_back();
        }
    }

    /// Navigate forward in history, if possible.
    pub fn go_forward(&self) {
        if let Some(b) = self.browser() {
            b.go_forward();
        }
    }

    /// Returns true if the browser can go back.
    pub fn can_go_back(&self) -> bool {
        self.browser().is_some_and(|b| b.can_go_back() != 0)
    }

    /// Returns true if the browser can go forward.
    pub fn can_go_forward(&self) -> bool {
        self.browser().is_some_and(|b| b.can_go_forward() != 0)
    }

    /// Returns true if the browser is currently loading.
    pub fn is_loading(&self) -> bool {
        self.browser().is_some_and(|b| b.is_loading() != 0)
    }

    /// Returns the current URL of the main frame.
    pub fn url(&self) -> String {
        self.browser()
            .and_then(|b| b.main_frame())
            .map(|f| {
                let c: CefString = (&f.url()).into();
                c.to_string()
            })
            .unwrap_or_default()
    }

    /// Execute JavaScript in the main frame.
    pub fn execute_javascript(&self, code: &str, script_url: &str, start_line: i32) {
        if let Some(frame) = self.browser().and_then(|b| b.main_frame()) {
            let code = CefString::from(code);
            let script_url = CefString::from(script_url);

            frame.execute_java_script(Some(&code), Some(&script_url), start_line);
        }
    }

    /// Open DevTools for this browser.
    pub fn show_devtools(&self) {
        if let Some(h) = self.host() {
            h.show_dev_tools(None, None, None, None);
        }
    }

    /// Close DevTools if open.
    pub fn close_devtools(&self) {
        if let Some(h) = self.host() {
            h.close_dev_tools();
        }
    }

    /// Returns true if DevTools is currently open for this browser.
    pub fn has_devtools(&self) -> bool {
        self.host().is_some_and(|h| h.has_dev_tools() != 0)
    }
}

/// The running application, owned by the thread that started it.
///
/// Not `Send`; CEF shuts down on the thread that initialized it and dropping
/// an `AppInstance` shuts CEF down. Use [`AppInstance::handle`] from other
/// threads.
pub struct AppInstance {
    handle: AppHandle,
    _ui_thread: PhantomData<*const ()>,
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

    /// Returns true once the application has ended; see
    /// [`AppHandle::should_shutdown`].
    pub fn should_shutdown(&self) -> bool {
        self.handle.should_shutdown()
    }

    /// Creates a new top-level window with an embedded browser.
    pub fn create_window(&self, options: WindowOptions) -> Result<WindowId, RuntimeError> {
        let mut client = KuroganeClient::new(self.handle.inner.services.clone(), BrowserType::Main);

        let window_id = {
            let mut reg = self.handle.inner.services.window_registry.lock().unwrap();
            reg.allocate_id()
        };

        let mut bv_delegate = KuroganeBrowserViewDelegate::new(
            self.handle.inner.services.browser_registry.clone(),
            self.handle.inner.services.window_registry.clone(),
            Some(window_id),
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
    /// The loop runs until the application's last browser has closed:
    /// after [`AppHandle::shutdown`] (from any thread), its last window
    /// closing, or Ctrl+C. After the loop exits, cef::shutdown() is called on
    /// the current (UI) thread.
    ///
    /// Not for an application given an [`App::scheduler`](crate::App::scheduler):
    /// the scheduler turns on CEF's external message pump, under which this
    /// loop returns at once and cef::shutdown() would run under a window
    /// still opening. Such an application calls [`AppInstance::pump`] from its
    /// own loop until [`AppInstance::should_shutdown`], then
    /// [`AppInstance::shutdown`].
    pub fn run(self) -> Result<(), RuntimeError> {
        // An application that ended before its loop started, with nothing
        // open, has nothing left to quit the loop
        if !self.should_shutdown() {
            run_message_loop();
        }

        debug!("Message loop exited");
        self.shutdown();

        Ok(())
    }

    /// Perform orderly CEF shutdown.
    ///
    /// Sets the shutdown signal and calls cef::shutdown() on the UI thread.
    /// Safe to call multiple times. Subsequent calls are no-ops.
    ///
    /// Unlike [`AppHandle::shutdown`], this shuts down the CEF runtime itself.
    /// All browsers must already be closed; [`AppHandle::should_shutdown`] is
    /// true at that point.
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
    /// The runtime must have been started with App::start_embedded,
    /// and AppInstance::pump must continue to be called regularly for
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
    /// The runtime must have been started with App::start_embedded.
    pub fn create_child_browser_with_request_context(
        &self,
        parent: *mut std::ffi::c_void,
        bounds: BrowserBounds,
        url: &str,
        rc_settings: &cef::RequestContextSettings,
    ) -> Option<BrowserHandle> {
        // Without its own context the browser would share the global cookie
        // and cache partition the caller asked to avoid
        let rc = cef::request_context_create_context(Some(rc_settings), None)?;
        self.create_child_browser_impl(parent, bounds, url, Some(rc))
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

        let mut client = KuroganeClient::new(self.handle.inner.services.clone(), BrowserType::Main);

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
fn initialize_cef(spec: RuntimeSpec, router: Arc<IpcRouter>) -> Result<RuntimeState, RuntimeError> {
    #[cfg(target_os = "macos")]
    crate::platform::macos::init_ns_app(spec.sandbox_mode)?;

    // The first call fixes the CEF API version for the whole process; the
    // Windows sandbox check compares hashes under this version later
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
    let mut app: App = KuroganeApp::create(services.clone(), spec.clone());

    // The same value has to reach both CEF entry points
    let sandbox_info = crate::sandbox::cef_sandbox_info(spec.sandbox_mode);

    debug!("Executing subprocess dispatch");
    execute_subprocesses(&args, &mut app, sandbox_info);

    let layout = resolve_layout(spec.profile_id)?;
    crate::sandbox::preflight(spec.sandbox_mode, &layout.cef_root)?;

    let external_message_pump = spec.scheduler.is_some();
    let settings = build_settings(
        &layout,
        spec.persist_session_cookies,
        external_message_pump,
        spec.sandbox_mode,
    );

    debug!("Initializing CEF");

    if initialize(
        Some(args.as_main_args()),
        Some(&settings),
        Some(&mut app),
        sandbox_info,
    ) != 1
    {
        // CEF runs one instance per profile. A launch that finds this
        // application running has handed it its command line and, like a
        // helper process, has nothing left to do
        let notified = sys::cef_resultcode_t::CEF_RESULT_CODE_NORMAL_EXIT_PROCESS_NOTIFIED as i32;

        if get_exit_code() == notified {
            debug!("Application already running; this launch was handed over to it");
            std::process::exit(0);
        }

        return Err(RuntimeError::CefInitializeFailed);
    }

    debug!("CEF initialized");

    #[cfg(target_os = "macos")]
    crate::platform::macos::setup_app_delegate();

    // Only install Ctrl+C handler if CEF Views owns the window (non-embedded mode)
    // In embedded mode the host application manages its own lifecycle
    if spec.mode == RuntimeMode::Views {
        debug!("Installing shutdown handler");
        install_ctrlc_handler(services.clone());
    }

    Ok(RuntimeState {
        services,
        ui_thread_id: std::thread::current().id(),
    })
}

impl RuntimeBootstrap {
    /// Initialize CEF and return an AppInstance without entering a message loop.
    ///
    /// In [`RuntimeMode::Embedded`] the host application owns window creation
    /// and lifecycle, so CEF Views creates no window.
    pub(crate) fn start(
        spec: RuntimeSpec,
        router: Arc<IpcRouter>,
    ) -> Result<AppInstance, RuntimeError> {
        let state = initialize_cef(spec, router)?;
        let handle = AppHandle {
            inner: Arc::new(AppHandleInner {
                services: state.services,
                ui_thread_id: state.ui_thread_id,
                cef_shutdown_called: AtomicBool::new(false),
            }),
        };

        Ok(AppInstance {
            handle,
            _ui_thread: PhantomData,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(target_os = "windows")]
    fn windows_names_no_subprocess_executable() {
        // CEF reads any browser_subprocess_path on Windows as "the helpers are
        // a separate executable", which its sandbox does not support, and
        // turns the sandbox off without reporting it. Helpers relaunch this
        // executable regardless, so the setting buys nothing and costs the
        // sandbox.
        assert_eq!(
            subprocess_path(std::path::Path::new(r"C:\app\myapp.exe")),
            None,
            "naming a subprocess executable silently disables the Windows sandbox"
        );
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn linux_names_its_own_executable() {
        // Linux has no such rule, and the helpers are this same binary
        assert_eq!(
            subprocess_path(std::path::Path::new("/app/myapp")),
            Some(std::path::PathBuf::from("/app/myapp"))
        );
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn unbundled_macos_names_its_own_executable() {
        // Outside an app bundle there are no helper apps, so helpers are this
        // same binary, as on Linux
        assert_eq!(
            subprocess_path(std::path::Path::new("/app/myapp")),
            Some(std::path::PathBuf::from("/app/myapp"))
        );
    }

    #[test]
    fn the_launched_process_is_the_browser_process() {
        assert!(browser_process_from_args([
            "/Apps/MyApp.app/Contents/MacOS/myapp"
        ]));
    }

    #[test]
    fn a_typed_process_is_a_subprocess() {
        for role in [
            "--type=renderer",
            "--type=gpu-process",
            "--type=utility",
            "--type=zygote",
        ] {
            assert!(
                !browser_process_from_args(["/path/to/helper", role, "--no-sandbox"]),
                "{role} is a subprocess"
            );
        }
    }

    #[test]
    fn an_unrelated_flag_does_not_make_it_a_subprocess() {
        assert!(browser_process_from_args([
            "/path/to/myapp",
            "--typewriter",
            "--type-check"
        ]));
    }
}
