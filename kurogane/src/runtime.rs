use cef::{args::Args, sys::cef_window_handle_t, *};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::marker::PhantomData;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::cef_app::KuroganeApp;
use crate::client::KuroganeClient;
use crate::error::RuntimeError;
use crate::browser_registry::{BrowserId, BrowserMetadata, BrowserType};
use crate::registry::Registry;
use crate::window_registry::{WindowId, WindowMetadata};
use crate::window::{Placement, open_browser_window};
use kurogane_layout::{DetectError, DiscoveryMode, detect_cef_root, validate_cef_runtime, profile_dir};
use crate::ipc::IpcRouter;
use crate::ipc::transport::message::RendererSandbox;
use crate::spec::{RuntimeMode, RuntimeSpec, SandboxMode};
use tracing::{debug, warn};

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

    let detected = detect_cef_root().map_err(cef_not_found)?;

    let invalid = |source: Box<dyn std::error::Error + Send + Sync>| {
        unusable_cef(detected.mode, detected.root.clone(), source)
    };
    validate_cef_runtime(&detected.root).map_err(|e| invalid(Box::new(e)))?;
    let cef_root = detected
        .root
        .canonicalize()
        .map_err(|e| invalid(Box::new(e)))?;

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

/// Names a Chromium runtime that cannot be used: a bundle's own leaves the
/// bundle incomplete, since a bundle runs no other; any other is an invalid
/// installation.
pub(crate) fn unusable_cef(
    mode: DiscoveryMode,
    path: std::path::PathBuf,
    source: Box<dyn std::error::Error + Send + Sync>,
) -> RuntimeError {
    match mode {
        DiscoveryMode::Bundled => RuntimeError::IncompleteBundle { path, source },
        DiscoveryMode::BesideExecutable | DiscoveryMode::EnvironmentOverride => {
            RuntimeError::InvalidCefInstallation { path, source }
        }
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

/// Closes every browser on Ctrl+C, as closing the windows by hand would.
///
/// ctrlc keeps the handler until the process exits, so it holds the runtime
/// weakly: it must not keep the application's state alive once it has ended.
fn install_ctrlc_handler(app: &AppHandle) {
    let runtime = app.downgrade();
    // ctrlc runs the handler on its one "ctrl-c" thread, one signal at a
    // time, so a plain flag is enough
    let mut quitting = false;

    let installed = ctrlc::set_handler(move || {
        debug!("SIGINT received");

        // Only act on the first signal (dev hammers Ctrl+C twice)
        if std::mem::replace(&mut quitting, true) {
            debug!("Shutdown already in progress");
            return;
        }

        debug!("Scheduling browser shutdown on UI thread");

        // Unload handlers still run, as for a window closed by hand; nothing
        // to close once the application has ended
        if let Some(app) = AppHandle::upgrade(&runtime) {
            app.request(Close::Everything { force: false });
        }
    });

    // A host that installed its own handler keeps it; the app still closes
    // normally, only not on Ctrl+C
    if let Err(err) = installed {
        warn!("Ctrl+C will not close the app: {err}");
    }
}

/// Closes all browsers, then any remaining Views windows. UI thread only.
///
/// Shutdown completes from the last browser's `OnBeforeClose` callback. If
/// there are no browsers or windows, it completes immediately. A window whose
/// browser is still being created remains open for this purpose.
///
/// `force` uses `CloseBrowser(true)`. It also closes unlinked Views windows
/// directly, whose browser is not created yet or has closed; linked windows
/// close with their browser.
fn close_all(app: &AppHandle, force: bool) {
    let no_browsers = app.registry().browsers.is_empty();
    let no_windows = app.registry().windows.count() == 0;
    if no_browsers && no_windows {
        // No browser's close is coming to end the application
        app.all_browsers_closed();
        return;
    }

    // Close all browsers first; in Views mode this cascades to close their parent windows
    // Embedded mode has no Views windows
    close_browsers(app, force);

    // In a forced close, linked windows close with their browser. Close only
    // unlinked windows directly.
    let windows = {
        let reg = app.registry();
        if force {
            reg.windows.unlinked()
        } else {
            reg.windows.all()
        }
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

/// Asks every live browser to close. UI thread only; [`AppHandle::request`]
/// brings it there.
fn close_browsers(app: &AppHandle, force: bool) {
    let browsers: Vec<Browser> = {
        let reg = app.registry();
        reg.browsers
            .iter()
            .map(|(_, s)| s.browser.clone())
            .collect()
    };

    for browser in browsers {
        if let Some(host) = browser.host() {
            debug!("closing browser cef_id={}", browser.identifier());
            host.close_browser(force as i32);
        }
    }
}

/// What a close request closes.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Close {
    /// Every browser, then the windows `close_all` closes directly. The last
    /// `OnBeforeClose` ends the application, or the request itself when
    /// nothing is open.
    Everything { force: bool },
    /// Every browser; `force` skips the pages' unload handlers.
    Browsers { force: bool },
    /// Every Views window. Each asks its browser to close first.
    Windows,
}

wrap_task! {
    struct CloseTask {
        app: AppHandle,
        what: Close,
    }

    impl Task {
        fn execute(&self) {
            // CEF runs this on its UI thread, so request acts at once, once it
            // has checked again that CEF has not begun shutting down since
            self.app.request(self.what);
        }
    }
}

/// Closes `what`. UI thread only; [`AppHandle::request`] brings it there.
fn close(app: &AppHandle, what: Close) {
    match what {
        Close::Everything { force } => close_all(app, force),
        Close::Browsers { force } => close_browsers(app, force),
        Close::Windows => {
            // The guard ends with the statement: Window::close runs CanClose
            let windows = app.registry().windows.all();
            close_windows(windows);
        }
    }
}

/// What the running application shares between CEF's callbacks, tasks and
/// the application's handles, which hold it through an [`AppHandle`]. The
/// Ctrl+C handler and the macOS `terminate:` override last as long as the
/// process, so they hold it weakly: it goes with the last handle.
pub(crate) struct RuntimeServices {
    router: IpcRouter,
    registry: Mutex<Registry>,
    /// The thread that initialized CEF: CEF's UI thread, since Kurogane
    /// never sets `multi_threaded_message_loop`
    ui_thread: std::thread::ThreadId,
    /// AppInstance::run is inside CEF's message loop, the only loop Kurogane
    /// may quit. Only the UI thread reads and writes it
    in_run_loop: AtomicBool,
    /// Every browser has closed: set by the last OnBeforeClose, or by a
    /// close request that finds nothing open
    ended: AtomicBool,
    /// Set when AppInstance::shutdown begins
    cef_shut_down: AtomicBool,
    /// Whether the renderers run in Chromium's sandbox, which decides
    /// whether the browser copies their shared memory
    renderer_sandbox: RendererSandbox,
}

impl RuntimeServices {
    /// Services with nothing open, whose UI thread is `ui_thread`.
    fn new(
        router: IpcRouter,
        ui_thread: std::thread::ThreadId,
        renderer_sandbox: RendererSandbox,
    ) -> Self {
        Self {
            router,
            registry: Mutex::new(Registry::new()),
            ui_thread,
            in_run_loop: AtomicBool::new(false),
            ended: AtomicBool::new(false),
            cef_shut_down: AtomicBool::new(false),
            renderer_sandbox,
        }
    }
}

/// A rectangle: a position and a size.
///
/// In [`WindowOptions::bounds`] it is a window's place on the screen.
///
/// For [`AppInstance::create_child_browser`] and [`BrowserHandle::set_bounds`]
/// it is a browser's place inside its parent window, in that window's own
/// coordinates, which CEF applies unconverted: pixels from the top-left of the
/// client area on Windows and X11; on macOS, points in the parent `NSView`,
/// whose origin is its top-left corner when the view is flipped (winit's is)
/// and its bottom-left corner otherwise.
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

/// Shared lifecycle handle for a running Kurogane application.
///
/// Obtain one with [`AppInstance::handle`]; clones share the application.
///
/// # Threads
///
/// Usable from any thread. Queries copy what is open under a short lock.
/// A broadcast sends to its subscribers on the calling thread, which CEF
/// allows for frames in the browser process. Closing and ending run on CEF's
/// UI thread: at once when called there, posted there otherwise.
///
/// Once [`AppInstance::shutdown`] has begun, calls that would reach CEF do
/// nothing, since CEF takes no call after it has shut down.
#[derive(Clone)]
pub struct AppHandle {
    services: Arc<RuntimeServices>,
}

// Send and Sync come from the fields: this stops compiling if a field of
// either handle is not both
const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<AppHandle>();
    assert_send_sync::<BrowserHandle>();
};

impl AppHandle {
    /// The open browsers and windows.
    ///
    /// Only the UI thread changes them; any thread may read them. Copy out
    /// what is needed and let the guard go before any CEF call that can run
    /// Kurogane's callbacks: creating a browser or a window, adding a
    /// browser's view to a window, or closing either. CEF may run those
    /// callbacks on this thread before the call returns, and they take this
    /// lock again, which deadlocks or panics. No other Kurogane lock is taken
    /// while the guard is held.
    ///
    /// A poisoned lock is recovered rather than turned into another panic: a
    /// panic under the guard can come only from a debug line, never from the
    /// middle of a map update, so the maps stay usable.
    pub(crate) fn registry(&self) -> MutexGuard<'_, Registry> {
        self.services
            .registry
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// The IPC router.
    pub(crate) fn router(&self) -> &IpcRouter {
        &self.services.router
    }

    /// Whether the renderers run in Chromium's sandbox.
    pub(crate) fn renderer_sandbox(&self) -> RendererSandbox {
        self.services.renderer_sandbox
    }

    /// The runtime held weakly, for what lasts as long as the process: the
    /// Ctrl+C handler and the macOS `terminate:` override.
    pub(crate) fn downgrade(&self) -> Weak<RuntimeServices> {
        Arc::downgrade(&self.services)
    }

    /// The handle of a runtime held weakly, while anything else still holds
    /// it.
    pub(crate) fn upgrade(runtime: &Weak<RuntimeServices>) -> Option<Self> {
        runtime.upgrade().map(|services| Self { services })
    }

    /// Ends the application once no browser is open: CEF asks an application
    /// to exit only after OnBeforeClose has run for every browser. Quits the
    /// message loop only while [`AppInstance::run`] is in it; an application
    /// that pumps CEF itself watches [`AppHandle::should_shutdown`] instead.
    /// UI thread.
    pub(crate) fn all_browsers_closed(&self) {
        debug!("No browser is open; the application has ended");
        self.services.ended.store(true, Ordering::Release);
        if self.services.in_run_loop.load(Ordering::Relaxed) {
            quit_message_loop();
        }
    }

    /// Returns whether this is CEF's UI thread: the thread that ran
    /// CefInitialize, since Kurogane never sets `multi_threaded_message_loop`
    /// (cef_types.h:1753-1755). Compares thread ids and asks CEF nothing, so
    /// it holds before CefInitialize and after CefShutdown alike.
    fn on_ui_thread(&self) -> bool {
        std::thread::current().id() == self.services.ui_thread
    }

    /// Returns true once `AppInstance::shutdown` has begun, after which
    /// nothing may reach CEF (cef_app.h:97-103). Asks CEF nothing.
    ///
    /// A call on another thread can read false just before the flag is set
    /// and reach CEF while CefShutdown runs. Only a lock held across every
    /// CEF call would close that gap, and CEF calls back into Kurogane on the
    /// same thread. When every browser has closed first, as CEF requires,
    /// such a call can only be the post of a close request. A post that races
    /// CefShutdown is outside CEF's contract; if it runs, CloseTask checks
    /// this flag again.
    fn cef_is_down(&self) -> bool {
        self.services.cef_shut_down.load(Ordering::Acquire)
    }

    /// Closes `what` on CEF's UI thread: at once when called there, posted
    /// there otherwise, as cefsimple's CloseAllBrowsers does. Does nothing
    /// once CEF has begun shutting down.
    pub(crate) fn request(&self, what: Close) {
        if self.cef_is_down() {
            debug!("CEF has shut down; ignoring {what:?}");
        } else if self.on_ui_thread() {
            close(self, what);
        } else {
            let mut task = CloseTask::new(self.clone(), what);
            if post_task(ThreadId::UI, Some(&mut task)) == 0 {
                debug!("CEF refused {what:?}; it did not run");
            }
        }
    }

    /// Ends the application by closing all browsers.
    ///
    /// The application ends after the last browser closes. Calling this from
    /// another thread posts the close to the UI thread. The call does not wait
    /// for the browsers to close. Does nothing once [`AppInstance::shutdown`]
    /// has begun.
    pub fn shutdown(&self) {
        debug!("AppHandle::shutdown: closing every browser");
        self.request(Close::Everything { force: true });
    }

    /// Returns whether the application has ended.
    ///
    /// Becomes true once the last browser has closed, whatever closed it: its
    /// window, [`AppHandle::shutdown`], Ctrl+C or macOS asking the
    /// application to quit. Any of those three that finds nothing open ends
    /// it at once. Also true once [`AppInstance::shutdown`] has begun. An
    /// application that pumps CEF itself keeps pumping until then.
    pub fn should_shutdown(&self) -> bool {
        self.services.ended.load(Ordering::Acquire)
            || self.services.cef_shut_down.load(Ordering::Acquire)
    }

    /// Broadcast an event to all renderers subscribed to event.
    ///
    /// The event is delivered asynchronously to every active subscription for the
    /// given event name. This method is thread-safe and returns immediately after
    /// queuing the event for delivery. Does nothing once
    /// [`AppInstance::shutdown`] has begun.
    pub fn broadcast(&self, event: &str, data: &[u8]) {
        // Subscriptions hold CEF frames
        if self.cef_is_down() {
            return;
        }
        self.router().event.broadcast(event, data);
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
        self.registry().browsers.count()
    }

    /// Number of currently open windows.
    pub fn window_count(&self) -> usize {
        self.registry().windows.count()
    }

    /// IDs of all open windows.
    pub fn window_ids(&self) -> Vec<WindowId> {
        let reg = self.registry();
        reg.windows.iter().map(|(id, _)| *id).collect()
    }

    /// Close all open windows.
    ///
    /// Safe to call from any thread: CEF's windows close only on the UI
    /// thread, so a call from elsewhere is posted there. Does nothing once
    /// [`AppInstance::shutdown`] has begun.
    pub fn close_all_windows(&self) {
        self.request(Close::Windows);
    }

    /// Close all live browser instances.
    ///
    /// The call does not wait: a browser counts in [`AppHandle::browser_count`]
    /// until CEF has finished closing it, so an application that pumps CEF
    /// keeps pumping until the count is 0. A browser embedded in the
    /// application's own window closes without asking that window to close.
    /// A call from another thread is posted to the UI thread. Does nothing
    /// once [`AppInstance::shutdown`] has begun.
    pub fn close_all_browsers(&self, force: bool) {
        self.request(Close::Browsers { force });
    }

    /// Look up the window that hosts a given browser.
    ///
    /// None once the browser has closed, even while CEF is still closing
    /// its window.
    pub fn find_window_by_browser(&self, browser_id: BrowserId) -> Option<WindowId> {
        self.registry().windows.window_id_for_browser(browser_id)
    }

    /// Metadata for all live browsers.
    pub fn browsers(&self) -> Vec<(BrowserId, BrowserMetadata)> {
        let reg = self.registry();
        reg.browsers
            .iter()
            .map(|(id, s)| (*id, s.metadata.clone()))
            .collect()
    }

    /// Metadata for all open windows.
    pub fn windows(&self) -> Vec<(WindowId, WindowMetadata)> {
        let reg = self.registry();
        reg.windows
            .iter()
            .map(|(id, s)| (*id, s.metadata.clone()))
            .collect()
    }

    /// Parent of a given browser.
    pub fn browser_parent(&self, id: BrowserId) -> Option<BrowserId> {
        self.registry().browsers.browser_parent(id)
    }

    /// Opener of a given browser.
    pub fn browser_opener(&self, id: BrowserId) -> Option<BrowserId> {
        self.registry().browsers.browser_opener(id)
    }

    /// All children of the given parent browser.
    pub fn children_of(&self, id: BrowserId) -> Vec<BrowserId> {
        self.registry().browsers.children_of(id)
    }

    /// Browser hosted in the given window.
    ///
    /// None until the window's browser has been created, and once it has
    /// closed.
    pub fn browser_for_window(&self, id: WindowId) -> Option<BrowserId> {
        self.registry().windows.browser_for_window(id)
    }

    /// Creates a BrowserHandle for a registered browser, if it exists.
    ///
    /// Returns None if no browser with the given BrowserId is registered, or
    /// once [`AppInstance::shutdown`] has begun.
    pub fn get_browser_handle(&self, id: BrowserId) -> Option<BrowserHandle> {
        if self.cef_is_down() {
            return None;
        }
        let reg = self.registry();
        if reg.browsers.get(id).is_some() {
            Some(BrowserHandle {
                id,
                app: self.clone(),
            })
        } else {
            None
        }
    }
}

#[cfg(test)]
impl AppHandle {
    /// A handle to an application CEF has not seen; the calling thread is its UI thread.
    pub(crate) fn detached() -> Self {
        use std::collections::HashMap;

        let router = IpcRouter::new(
            crate::ipc::RequestResponseSubsystem::new(HashMap::new(), HashMap::new()),
            crate::ipc::EventSubsystem::new(),
            crate::ipc::StreamSubsystem::new(HashMap::new()),
            // No application origin is associated with a detached handle; only explicitly
            // permitted names are reachable
            crate::acl::CommandAcl::new(crate::acl::Origin::OPAQUE),
        );
        Self {
            services: Arc::new(RuntimeServices::new(
                router,
                std::thread::current().id(),
                RendererSandbox::Sandboxed,
            )),
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

/// The CEF parent window for `parent`: its HWND on Windows, its NSView on
/// macOS, its X11 window on Linux (Xlib or XCB). CEF parents a browser in
/// nothing else, so any other handle, a Wayland surface included, is refused.
fn parent_window(parent: &impl HasWindowHandle) -> Result<cef_window_handle_t, RuntimeError> {
    let handle = parent
        .window_handle()
        .map_err(|_| RuntimeError::UnsupportedParentWindow)?;
    match handle.as_raw() {
        #[cfg(target_os = "windows")]
        RawWindowHandle::Win32(window) => Ok(cef::sys::HWND(window.hwnd.get() as *mut _)),
        #[cfg(target_os = "macos")]
        RawWindowHandle::AppKit(window) => Ok(window.ns_view.as_ptr() as cef_window_handle_t),
        #[cfg(target_os = "linux")]
        RawWindowHandle::Xlib(window) => Ok(window.window),
        #[cfg(target_os = "linux")]
        RawWindowHandle::Xcb(window) => Ok(window.window.get().into()),
        _ => Err(RuntimeError::UnsupportedParentWindow),
    }
}

/// A browser of the running application.
///
/// The handle names the browser by its [`BrowserId`] and looks it up on each
/// call. Once the browser has closed, or once [`AppInstance::shutdown`] has
/// begun, the calls that would reach it do nothing and its queries answer
/// false, or an empty URL.
///
/// # Threads
///
/// Usable from any thread, as CEF allows for a browser in the browser
/// process. [`set_bounds`](Self::set_bounds) and
/// [`has_devtools`](Self::has_devtools) are the exceptions: they run only on
/// the thread that started the application, which is CEF's UI thread, and
/// panic elsewhere.
pub struct BrowserHandle {
    id: BrowserId,
    app: AppHandle,
}

// By hand: the AppHandle it holds has no Debug
impl std::fmt::Debug for BrowserHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BrowserHandle")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

impl BrowserHandle {
    /// Panics off CEF's UI thread. CEF allows a browser, its host and its
    /// frames on any thread of the browser process unless a method says
    /// otherwise (cef_browser.h:56-59 and 276-279, cef_frame.h:54-57); of the
    /// calls here only HasDevTools does (cef_browser.h:594-598). set_bounds
    /// moves the browser's native view, which platform::embed touches only on
    /// the UI thread.
    #[track_caller]
    fn assert_ui_thread(&self) {
        assert!(
            self.app.on_ui_thread(),
            "BrowserHandle::set_bounds and has_devtools run only on CEF's UI thread, the thread that started the application"
        );
    }

    /// Returns the browser this handle names, while it is open.
    fn browser(&self) -> Option<Browser> {
        // No browser is open once CEF has begun shutting down
        if self.app.cef_is_down() {
            return None;
        }
        let reg = self.app.registry();
        reg.browsers.get(self.id).map(|s| s.browser.clone())
    }

    /// Returns the host of the browser this handle names, while it is open.
    fn host(&self) -> Option<BrowserHost> {
        self.browser()?.host()
    }

    /// The id of the browser this handle names.
    pub fn id(&self) -> BrowserId {
        self.id
    }

    /// Closes the browser.
    ///
    /// With `force` false the page's unload handlers run first and may cancel
    /// the close. The call does not wait: the browser is gone once CEF has
    /// finished closing it. A browser embedded in the application's own
    /// window closes without asking that window to close.
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

    /// Moves and resizes a browser made by
    /// [`AppInstance::create_child_browser`] inside its parent window.
    ///
    /// `bounds` are in the parent's coordinates, as at creation (see
    /// [`BrowserBounds`]); a width or height below one counts as one. On
    /// Windows and Linux Chromium places such a browser once and does not
    /// follow its parent, so the application calls this whenever the
    /// browser's place changes: for a browser that fills its window, on every
    /// resize. On macOS the browser stretches with its parent by itself, and
    /// this places it anywhere.
    ///
    /// Does nothing for any other browser, and nothing once the browser has
    /// closed.
    ///
    /// # Panics
    ///
    /// Off the thread that started the application, which is CEF's UI thread:
    /// the browser's native window is moved only there.
    #[track_caller]
    pub fn set_bounds(&self, bounds: BrowserBounds) {
        self.assert_ui_thread();
        let Some(browser) = self.browser() else {
            return;
        };
        let Some(host) = browser.host() else {
            return;
        };
        // A browser in a window Kurogane or CEF made (the application's,
        // a popup's, DevTools') is sized with that window
        if host.has_view() != 0 || browser.is_popup() != 0 {
            return;
        }
        crate::platform::embed::set_child_window_bounds(self.id, host.window_handle(), bounds);
    }

    /// Tells the browser that the window hosting it is about to move or
    /// resize. CEF uses this on Windows and Linux only.
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
    ///
    /// # Panics
    ///
    /// Off the thread that started the application, which is CEF's UI thread:
    /// CEF answers this only there.
    #[track_caller]
    pub fn has_devtools(&self) -> bool {
        self.assert_ui_thread();
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
    /// CEF runs its external message pump; the application has a scheduler
    #[cfg(target_os = "linux")]
    external_pump: bool,
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
        // Under the external pump nothing else dispatches Chromium's X11 and
        // Wayland events on Linux. Native events first, then the work they
        // post, as Chromium's own glib pump orders them
        #[cfg(target_os = "linux")]
        if self.external_pump {
            crate::platform::linux::dispatch_glib_events();
        }
        do_message_loop_work();
    }

    /// Returns true once the application has ended; see
    /// [`AppHandle::should_shutdown`].
    pub fn should_shutdown(&self) -> bool {
        self.handle.should_shutdown()
    }

    /// Creates a new top-level window with an embedded browser.
    pub fn create_window(&self, options: WindowOptions) -> Result<WindowId, RuntimeError> {
        let bounds = options.bounds;
        let placement = Placement::Main {
            bounds: Rect {
                x: bounds.x,
                y: bounds.y,
                width: bounds.width,
                height: bounds.height,
            },
            show_state: options.show_state.into(),
        };
        open_browser_window(&self.handle, &options.url, placement)
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
            let services = &self.handle.services;
            // The last browser's close quits this loop, and only this one
            services.in_run_loop.store(true, Ordering::Relaxed);
            run_message_loop();
            services.in_run_loop.store(false, Ordering::Relaxed);
        }

        debug!("Message loop exited");
        self.shutdown();

        Ok(())
    }

    /// Perform orderly CEF shutdown.
    ///
    /// Calls cef::shutdown() on the UI thread; [`AppHandle::should_shutdown`]
    /// is true from here on. Safe to call multiple times. Subsequent calls are
    /// no-ops.
    ///
    /// Unlike [`AppHandle::shutdown`], this shuts down the CEF runtime itself.
    /// All browsers must already be closed; [`AppHandle::should_shutdown`] is
    /// true at that point.
    ///
    /// From here on, [`AppHandle`] and [`BrowserHandle`] calls that would
    /// reach CEF do nothing. Finish using them on other threads first: a call
    /// made there at this moment can still reach CEF.
    pub fn shutdown(&self) {
        let services = &self.handle.services;
        // Before CefShutdown, so from here no handle reaches CEF
        if services.cef_shut_down.swap(true, Ordering::SeqCst) {
            return;
        }

        debug!("Shutting down Kurogane runtime");
        shutdown();
        debug!("Kurogane runtime shutdown complete");
    }

    /// Creates a Chromium browser hosted inside an existing native window.
    ///
    /// The browser is a native child window of `parent`, placed at `bounds`
    /// in the parent's coordinates (see [`BrowserBounds`]).
    /// [`BrowserHandle::set_bounds`] moves it later.
    ///
    /// `parent` is the host's window as it is, a winit `Window` for one:
    /// anything that hands out a [`raw_window_handle`] window handle. It must
    /// outlive the browser, which the host closes first (docs/winit.md).
    ///
    /// The runtime must have been started with App::start_embedded,
    /// and AppInstance::pump must continue to be called regularly for
    /// Chromium to process events.
    ///
    /// # Errors
    ///
    /// [`RuntimeError::UnsupportedParentWindow`] when `parent` is not a
    /// Win32 window, an AppKit view or an X11 window (a Wayland surface, for
    /// one), and [`RuntimeError::BrowserCreationFailed`] when CEF creates no
    /// browser.
    pub fn create_child_browser(
        &self,
        parent: &impl HasWindowHandle,
        bounds: BrowserBounds,
        url: &str,
    ) -> Result<BrowserHandle, RuntimeError> {
        self.create_child_browser_impl(parent, bounds, url, None)
    }

    /// Creates a child browser with a custom request context (separate cookie/cache partition).
    ///
    /// Same as create_child_browser but accepts RequestContextSettings to control
    /// the cache partition, cookie persistence and accept language for this browser.
    ///
    /// The runtime must have been started with App::start_embedded.
    ///
    /// # Errors
    ///
    /// As [`create_child_browser`](Self::create_child_browser), and
    /// [`RuntimeError::BrowserCreationFailed`] when CEF creates no request
    /// context from `rc_settings`.
    pub fn create_child_browser_with_request_context(
        &self,
        parent: &impl HasWindowHandle,
        bounds: BrowserBounds,
        url: &str,
        rc_settings: &cef::RequestContextSettings,
    ) -> Result<BrowserHandle, RuntimeError> {
        // Without its own context the browser would share the global cookie
        // and cache partition the caller asked to avoid
        let rc = cef::request_context_create_context(Some(rc_settings), None)
            .ok_or(RuntimeError::BrowserCreationFailed)?;
        self.create_child_browser_impl(parent, bounds, url, Some(rc))
    }

    fn create_child_browser_impl(
        &self,
        parent: &impl HasWindowHandle,
        bounds: BrowserBounds,
        url: &str,
        request_context: Option<cef::RequestContext>,
    ) -> Result<BrowserHandle, RuntimeError> {
        let info = WindowInfo {
            runtime_style: RuntimeStyle::ALLOY,
            ..WindowInfo::default()
        }
        .set_as_child(
            parent_window(parent)?,
            &Rect {
                x: bounds.x,
                y: bounds.y,
                width: bounds.width,
                height: bounds.height,
            },
        );

        let mut client = KuroganeClient::new(self.handle.clone(), BrowserType::Main);

        let mut rc = request_context;
        let browser = browser_host_create_browser_sync(
            Some(&info),
            Some(&mut client),
            Some(&CefString::from(url)),
            Some(&Default::default()),
            None,
            rc.as_mut(),
        )
        .ok_or(RuntimeError::BrowserCreationFailed)?;

        debug!("create_child_browser_impl cef_id={}", browser.identifier());

        // CreateBrowserSync delivers on_after_created, which registers the
        // browser, before it returns; the guard ends with the statement
        let id = self
            .handle
            .registry()
            .browsers
            .find_id_by_cef_id(browser.identifier())
            .ok_or(RuntimeError::BrowserCreationFailed)?;

        // set_bounds reaches the view through this, never through the handle
        #[cfg(target_os = "macos")]
        if let Some(host) = browser.host() {
            crate::platform::embed::remember_view(id, host.window_handle());
        }

        Ok(BrowserHandle {
            id,
            app: self.handle.clone(),
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
/// Returns the application's handle on success.
fn initialize_cef(spec: RuntimeSpec, router: IpcRouter) -> Result<AppHandle, RuntimeError> {
    // CEF's UI thread is the thread that initializes it, since Kurogane
    // never sets `multi_threaded_message_loop`
    let ui_thread = std::thread::current().id();

    #[cfg(target_os = "macos")]
    crate::platform::macos::init_ns_app(spec.sandbox_mode)?;

    // The first call fixes the CEF API version for the whole process; the
    // Windows sandbox check compares hashes under this version later
    let _ = api_hash(sys::CEF_API_VERSION_LAST, 0);

    debug!("Runtime initializing");

    let args = Args::new();

    let handle = AppHandle {
        services: Arc::new(RuntimeServices::new(
            router,
            ui_thread,
            RendererSandbox::of(spec.sandbox_mode),
        )),
    };

    // ONE app for ALL processes
    let mut app: App = KuroganeApp::create(handle.clone(), spec.clone());

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

    // Set once CEF runs, so a start that fails leaves it unset
    #[cfg(target_os = "macos")]
    crate::platform::macos::set_app(&handle);

    #[cfg(target_os = "macos")]
    crate::platform::macos::setup_app_delegate();

    // Preserve an existing application or host menu
    #[cfg(target_os = "macos")]
    crate::platform::macos::install_default_menu();

    // Only install Ctrl+C handler if CEF Views owns the window (non-embedded mode)
    // In embedded mode the host application manages its own lifecycle
    if spec.mode == RuntimeMode::Views {
        debug!("Installing shutdown handler");
        install_ctrlc_handler(&handle);
    }

    Ok(handle)
}

/// Initializes CEF and returns the application without entering a message
/// loop.
///
/// In [`RuntimeMode::Embedded`] the host application owns window creation
/// and lifecycle, so CEF Views creates no window.
pub(crate) fn start(spec: RuntimeSpec, router: IpcRouter) -> Result<AppInstance, RuntimeError> {
    #[cfg(target_os = "linux")]
    let external_pump = spec.scheduler.is_some();
    Ok(AppInstance {
        handle: initialize_cef(spec, router)?,
        #[cfg(target_os = "linux")]
        external_pump,
        _ui_thread: PhantomData,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A host window whose handle is `raw`.
    struct HostWindow(RawWindowHandle);

    impl HasWindowHandle for HostWindow {
        fn window_handle(
            &self,
        ) -> Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError> {
            // SAFETY: the handle is only mapped to CEF's parent type here,
            // never used as a window
            Ok(unsafe { raw_window_handle::WindowHandle::borrow_raw(self.0) })
        }
    }

    #[test]
    fn a_child_browser_is_parented_only_in_a_window_cef_embeds_in() {
        let wayland = raw_window_handle::WaylandWindowHandle::new(std::ptr::NonNull::dangling());
        assert!(matches!(
            parent_window(&HostWindow(wayland.into())),
            Err(RuntimeError::UnsupportedParentWindow)
        ));
        #[cfg(target_os = "windows")]
        {
            let hwnd = std::num::NonZeroIsize::new(0x2a).unwrap();
            let window = raw_window_handle::Win32WindowHandle::new(hwnd);
            assert!(parent_window(&HostWindow(window.into())).is_ok());
        }
        #[cfg(target_os = "macos")]
        {
            let view = std::ptr::NonNull::<std::ffi::c_void>::dangling();
            let window = raw_window_handle::AppKitWindowHandle::new(view);
            assert!(parent_window(&HostWindow(window.into())).is_ok());
        }
        #[cfg(target_os = "linux")]
        {
            let xlib = raw_window_handle::XlibWindowHandle::new(0x2a);
            assert_eq!(parent_window(&HostWindow(xlib.into())).ok(), Some(0x2a));
            let xcb =
                raw_window_handle::XcbWindowHandle::new(std::num::NonZeroU32::new(0x2b).unwrap());
            assert_eq!(parent_window(&HostWindow(xcb.into())).ok(), Some(0x2b));
        }
    }

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

    // ---- fake CEF objects ----
    //
    // A CEF object is a C structure of function pointers. A fake fills in
    // only the functions a test needs and counts every call made through
    // them; cef-rs answers a default for a function left out, without
    // calling anything. No test here loads CEF
    use std::ffi::c_int;
    use std::sync::atomic::AtomicUsize;

    use cef::rc::ConvertReturnValue;
    use cef::sys::{
        _cef_base_ref_counted_t, _cef_browser_host_t, _cef_browser_t, _cef_frame_t, _cef_window_t,
    };

    use crate::acl::Origin;
    use crate::ipc::FrameId;
    use crate::ipc::event::EventSubscription;

    /// A CEF structure the test made, and the calls made through it.
    #[repr(C)]
    struct Fake<T> {
        // First, so the structure and its base share the fake's address
        raw: T,
        calls: AtomicUsize,
    }

    /// Counts a call made through `object`.
    ///
    /// # Safety
    ///
    /// `object` points at the `raw` field of a live `Fake<T>`.
    unsafe fn count<T>(object: *mut T) {
        // SAFETY: `raw` is the first field of the repr(C) Fake<T> the caller
        // names, so the two share an address
        let fake = unsafe { &*object.cast::<Fake<T>>() };
        fake.calls.fetch_add(1, Ordering::SeqCst);
    }

    unsafe extern "C" fn add_ref<T>(base: *mut _cef_base_ref_counted_t) {
        // SAFETY: a CEF structure starts with its base, and every base given
        // to this function belongs to a Fake<T>
        unsafe { count(base.cast::<T>()) }
    }

    unsafe extern "C" fn release<T>(base: *mut _cef_base_ref_counted_t) -> c_int {
        // SAFETY: as in add_ref
        unsafe { count(base.cast::<T>()) };
        // A fake is never freed
        0
    }

    unsafe extern "C" fn called<T>(object: *mut T) {
        // SAFETY: cef-rs passes the structure it wraps, a Fake<T>'s
        unsafe { count(object) }
    }

    unsafe extern "C" fn no_host(browser: *mut _cef_browser_t) -> *mut _cef_browser_host_t {
        // SAFETY: as in called
        unsafe { count(browser) };
        std::ptr::null_mut()
    }

    unsafe extern "C" fn called_false<T>(object: *mut T) -> c_int {
        // SAFETY: as in called
        unsafe { count(object) };
        0
    }

    // Registering a browser reads its id; that is setup, not counted
    unsafe extern "C" fn identifier(_browser: *mut _cef_browser_t) -> c_int {
        1
    }

    /// Leaks a fake around `raw` and wraps it as cef-rs wraps what CEF returns.
    fn leak<T: 'static, W>(raw: T) -> (W, &'static AtomicUsize)
    where
        *mut T: ConvertReturnValue<W>,
    {
        let fake: &'static Fake<T> = Box::leak(Box::new(Fake {
            raw,
            calls: AtomicUsize::new(0),
        }));
        // cef-rs only reads the structure, and the count is atomic
        let object = std::ptr::from_ref(&fake.raw).cast_mut();
        (
            <*mut T as ConvertReturnValue<W>>::wrap_result(object),
            &fake.calls,
        )
    }

    fn fake_browser() -> (Browser, &'static AtomicUsize) {
        // SAFETY: a CEF structure is plain C data; all zeroes leave every
        // function out
        let mut raw: _cef_browser_t = unsafe { std::mem::zeroed() };
        raw.base.size = std::mem::size_of::<_cef_browser_t>();
        raw.base.add_ref = Some(add_ref::<_cef_browser_t>);
        raw.base.release = Some(release::<_cef_browser_t>);
        raw.get_identifier = Some(identifier);
        raw.get_host = Some(no_host);
        leak(raw)
    }

    fn fake_window() -> (Window, &'static AtomicUsize) {
        // SAFETY: as in fake_browser
        let mut raw: _cef_window_t = unsafe { std::mem::zeroed() };
        // A window is a panel is a view, which starts with the base
        let base = &mut raw.base.base.base;
        base.size = std::mem::size_of::<_cef_window_t>();
        base.add_ref = Some(add_ref::<_cef_window_t>);
        base.release = Some(release::<_cef_window_t>);
        raw.close = Some(called::<_cef_window_t>);
        leak(raw)
    }

    fn fake_frame() -> (Frame, &'static AtomicUsize) {
        // SAFETY: as in fake_browser
        let mut raw: _cef_frame_t = unsafe { std::mem::zeroed() };
        raw.base.size = std::mem::size_of::<_cef_frame_t>();
        raw.base.add_ref = Some(add_ref::<_cef_frame_t>);
        raw.base.release = Some(release::<_cef_frame_t>);
        // Gone, so a broadcast stops at it
        raw.is_valid = Some(called_false::<_cef_frame_t>);
        leak(raw)
    }

    /// A runtime with a browser, its window and a subscription to "tick".
    struct Opened {
        app: AppHandle,
        browser: BrowserId,
        window: WindowId,
        browser_calls: &'static AtomicUsize,
        window_calls: &'static AtomicUsize,
        frame_calls: &'static AtomicUsize,
    }

    impl Opened {
        fn new() -> Self {
            let app = AppHandle::detached();
            let (browser, browser_calls) = fake_browser();
            let (window, window_calls) = fake_window();
            let (frame, frame_calls) = fake_frame();
            let (id, window_id) = {
                let mut registry = app.registry();
                let id = registry
                    .browsers
                    .ensure_registered(&browser, BrowserType::Main, None);
                let window_id = registry.windows.allocate_id();
                registry.windows.insert(window_id, window, Some(id));
                (id, window_id)
            };
            let origin = Origin::parse("app://app").unwrap();
            app.router()
                .event
                .subscriptions
                .lock()
                .unwrap()
                .entry("tick".to_owned())
                .or_default()
                .push(EventSubscription {
                    id: 1,
                    frame,
                    browser_id: id,
                    frame_id: FrameId::new("1"),
                    origin: origin.clone(),
                    url_origin: origin,
                });
            drop(browser);
            // Setup is over: from here each count is what the handle did
            for calls in [browser_calls, window_calls, frame_calls] {
                calls.store(0, Ordering::SeqCst);
            }
            Self {
                app,
                browser: id,
                window: window_id,
                browser_calls,
                window_calls,
                frame_calls,
            }
        }

        fn calls(&self) -> [(&'static str, usize); 3] {
            [
                ("browser", self.browser_calls.load(Ordering::SeqCst)),
                ("window", self.window_calls.load(Ordering::SeqCst)),
                ("frame", self.frame_calls.load(Ordering::SeqCst)),
            ]
        }
    }

    /// Every AppHandle method, with the ids of what is open.
    fn use_every_method(app: &AppHandle, browser: BrowserId, window: WindowId) {
        app.shutdown();
        app.close_all_windows();
        app.close_all_browsers(false);
        app.close_all_browsers(true);
        app.broadcast("tick", b"1");
        app.broadcast_json("tick", &1);
        let _ = app.get_browser_handle(browser);
        let _ = app.should_shutdown();
        let _ = (app.browser_count(), app.window_count(), app.window_ids());
        let _ = (app.browsers(), app.windows());
        let _ = (
            app.find_window_by_browser(browser),
            app.browser_for_window(window),
        );
        let _ = (
            app.browser_parent(browser),
            app.browser_opener(browser),
            app.children_of(browser),
        );
    }

    /// Every BrowserHandle method CEF allows on any thread.
    fn use_any_thread_browser_methods(handle: &BrowserHandle) {
        let _ = handle.id();
        handle.close(false);
        handle.notify_move_or_resize_started();
        handle.navigate("app://app/");
        handle.reload();
        handle.reload_ignore_cache();
        handle.go_back();
        handle.go_forward();
        let _ = (
            handle.can_go_back(),
            handle.can_go_forward(),
            handle.is_loading(),
        );
        let _ = handle.url();
        handle.execute_javascript("1", "", 0);
        handle.show_devtools();
        handle.close_devtools();
    }

    /// Every BrowserHandle method. UI thread, which set_bounds and
    /// has_devtools assert.
    fn use_every_browser_method(handle: &BrowserHandle) {
        use_any_thread_browser_methods(handle);
        handle.set_bounds(BrowserBounds {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
        });
        let _ = handle.has_devtools();
    }

    #[test]
    fn the_ui_thread_is_the_one_that_made_the_runtime() {
        // Before CefInitialize CEF knows no UI thread, so asking it would say
        // no here (and crash on macOS, where no test loads CEF)
        let app = AppHandle::detached();
        assert!(app.on_ui_thread());
        std::thread::scope(|scope| {
            scope.spawn(|| assert!(!app.on_ui_thread()));
        });
    }

    #[test]
    fn nothing_reaches_cef_once_it_has_shut_down() {
        let opened = Opened::new();
        let app = &opened.app;
        app.services.cef_shut_down.store(true, Ordering::Release);
        let handle = BrowserHandle {
            id: opened.browser,
            app: app.clone(),
        };

        // On the UI thread, where a close would run at once
        use_every_method(app, opened.browser, opened.window);
        use_every_browser_method(&handle);
        assert!(app.get_browser_handle(opened.browser).is_none());
        // A task CEF runs after shutdown began checks again
        CloseTask::new(app.clone(), Close::Everything { force: true }).execute();

        // On another thread, where a close would be posted and a browser
        // handle calls CEF from there
        std::thread::scope(|scope| {
            scope.spawn(|| {
                use_every_method(app, opened.browser, opened.window);
                use_any_thread_browser_methods(&handle);
            });
        });

        for (what, calls) in opened.calls() {
            assert_eq!(
                calls, 0,
                "the handle reached the {what} after CEF shut down"
            );
        }
    }

    #[test]
    fn the_fakes_see_what_a_running_handle_reaches() {
        let opened = Opened::new();
        let app = &opened.app;

        // UI thread only: off it a close is a post, which needs a running CEF
        app.close_all_browsers(false);
        app.close_all_windows();
        app.broadcast("tick", b"1");
        for (what, calls) in opened.calls() {
            assert!(calls > 0, "the {what} saw nothing");
        }

        let handle = BrowserHandle {
            id: opened.browser,
            app: app.clone(),
        };
        opened.browser_calls.store(0, Ordering::SeqCst);
        handle.reload();
        assert!(
            opened.browser_calls.load(Ordering::SeqCst) > 0,
            "a browser handle looks its browser up"
        );

        // CEF allows a browser on any thread of the browser process
        opened.browser_calls.store(0, Ordering::SeqCst);
        std::thread::scope(|scope| {
            scope.spawn(|| handle.reload());
        });
        assert!(
            opened.browser_calls.load(Ordering::SeqCst) > 0,
            "a browser handle reaches its browser from another thread"
        );

        opened.window_calls.store(0, Ordering::SeqCst);
        CloseTask::new(app.clone(), Close::Windows).execute();
        assert!(
            opened.window_calls.load(Ordering::SeqCst) > 0,
            "a task closes the window"
        );
    }

    /// Runs `call` on a handle to an open browser, on a thread that is not
    /// the UI thread, and panics with its panic.
    fn off_the_ui_thread(call: impl FnOnce(&BrowserHandle) + Send) {
        let opened = Opened::new();
        let handle = BrowserHandle {
            id: opened.browser,
            app: opened.app.clone(),
        };
        std::thread::scope(|scope| {
            if let Err(panic) = scope.spawn(|| call(&handle)).join() {
                std::panic::resume_unwind(panic);
            }
        });
    }

    #[test]
    #[should_panic(expected = "CEF's UI thread")]
    fn has_devtools_panics_off_the_ui_thread() {
        // CEF answers HasDevTools only there (cef_browser.h:594-598)
        off_the_ui_thread(|handle| {
            let _ = handle.has_devtools();
        });
    }

    #[test]
    #[should_panic(expected = "CEF's UI thread")]
    fn set_bounds_panics_off_the_ui_thread() {
        // The native view it moves is touched only on the UI thread
        off_the_ui_thread(|handle| {
            handle.set_bounds(BrowserBounds {
                x: 0,
                y: 0,
                width: 1,
                height: 1,
            });
        });
    }

    #[test]
    fn a_close_request_with_nothing_open_ends_the_application() {
        // No browser's close is coming, so the request itself ends it; no
        // loop runs, so nothing is quit
        let app = AppHandle::detached();
        close_all(&app, true);
        assert!(app.should_shutdown());
    }

    #[test]
    fn a_closed_browser_leaves_no_window_naming_it() {
        let app = AppHandle::detached();
        let (browser, _) = fake_browser();
        let (window, _) = fake_window();
        let (id, window_id) = {
            let mut registry = app.registry();
            let id = registry
                .browsers
                .ensure_registered(&browser, BrowserType::Main, None);
            let window_id = registry.windows.allocate_id();
            registry.windows.insert(window_id, window, Some(id));
            (id, window_id)
        };
        assert_eq!(app.find_window_by_browser(id), Some(window_id));

        // What OnBeforeClose does first
        let closed = app.registry().browser_closed(&browser);
        let closed = closed.expect("the browser was registered");
        assert!(closed.last);
        assert!(closed.stragglers.is_empty());

        // CEF destroys the window later; until then it names no browser
        assert_eq!(app.window_count(), 1);
        assert_eq!(app.find_window_by_browser(id), None);
        assert_eq!(app.browser_for_window(window_id), None);
    }

    #[test]
    fn a_panic_under_the_registry_lock_leaves_it_usable() {
        let app = AppHandle::detached();
        let holder = app.clone();
        let panicked: std::thread::Result<()> = std::thread::spawn(move || {
            let _registry = holder.registry();
            panic!("a panic while the registry is locked");
        })
        .join();
        assert!(panicked.is_err());

        // Poisoned, and every query still answers
        assert_eq!(app.browser_count(), 0);
        assert_eq!(app.window_count(), 0);
        assert!(app.window_ids().is_empty());
        assert!(app.browsers().is_empty());
        assert!(app.windows().is_empty());
        assert_eq!(app.find_window_by_browser(BrowserId::new(1)), None);
        assert!(app.children_of(BrowserId::new(1)).is_empty());
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
