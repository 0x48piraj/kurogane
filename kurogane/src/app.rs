//! High level application bootstrap API.
//!
//! This is the public developer entrypoint built on top of Runtime.
//! This helps in the abstraction of asset resolution, environment overrides and command registration.

use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use std::sync::Arc;
use serde_json::Value;
use std::collections::HashMap;
use cef::*;
use crate::app::resolver::ResolvedFrontend;
use crate::ipc::{
    IpcRouter, RequestResponseSubsystem, EventSubsystem, StreamSubsystem, StreamFactory, Responder,
    BinaryResponder, SyncHandler, AsyncHandler, IpcError,
};
use crate::runtime::{AppHandle, AppInstance};
use crate::error::{ConfigError, RuntimeError};
use crate::spec::{RuntimeSpec, RuntimeMode, SandboxMode};
use crate::scheme::{CustomScheme, SchemeHandler, validate_scheme_name};
use crate::chromium_flags::ChromiumFlag;
use crate::credentials::CredentialStorage;
use crate::gpu::GpuMode;
use crate::capability::{FilesystemBuilder, FsConfigError};
use crate::acl::Origin;
use crate::chrome_commands::{ChromeCommandRequest, CommandDecision};
use crate::context_menu::{ContextMenu, ContextMenuCommand};
use crate::downloads::{DownloadDecision, DownloadRequest};
use crate::permissions::{PermissionDecision, PermissionRequest};
use crate::hooks::Hooks;
use crate::keys::{KeyDecision, KeyPress};
use crate::navigation::{NavigationDecision, NavigationRequest};
use crate::new_window::{NewWindowDecision, NewWindowRequest};
use crate::window::WindowIdentity;
use crate::window_closing::WindowClosing;
use crate::window_options::WindowOptions;

mod resolver;

/// A request from CEF indicating when it next needs to be serviced.
///
/// Passed to the closure given to [`App::scheduler`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PumpRequest {
    /// CEF needs work immediately.
    Now,
    /// CEF needs work after the given delay.
    After(Duration),
}

impl PumpRequest {
    /// Returns when to call [`AppInstance::pump`] for a request made at
    /// `now`: `now` itself, or `now` plus the delay.
    ///
    /// A delay too long to add to `now` gives `now`; pumping early is safe.
    pub fn deadline(self, now: Instant) -> Instant {
        match self {
            Self::Now => now,
            Self::After(delay) => now.checked_add(delay).unwrap_or(now),
        }
    }
}

/// Callback type for pump scheduling.
///
/// CEF calls this whenever it wants AppInstance::pump to be called.
/// The integrator decides how to honour the request via a winit proxy, a glib timeout, a Tokio task, or anything else.
pub type PumpScheduler = Arc<dyn Fn(PumpRequest) + Send + Sync>;

/// A launch of the application while another instance is already running.
///
/// Chromium allows one instance per profile. The new launch passes its
/// arguments and working directory to the running instance and exits.
///
/// Passed to the closure given to [`App::on_second_instance`].
#[derive(Debug, Clone)]
pub struct SecondInstance {
    args: Vec<String>,
    switches: HashMap<String, String>,
    working_dir: Option<PathBuf>,
}

impl SecondInstance {
    /// Copies out the launch CEF handed over.
    pub(crate) fn from_launch(command_line: &CommandLine, working_dir: Option<&CefString>) -> Self {
        let mut args = CefStringList::new();
        command_line.arguments(Some(&mut args));

        let mut switches = CefStringMap::new();
        command_line.switches(Some(&mut switches));

        Self {
            args: args.into_iter().collect(),
            switches: switches.into_iter().collect(),
            working_dir: working_dir
                .map(|dir| dir.to_string())
                .filter(|dir| !dir.is_empty())
                .map(PathBuf::from),
        }
    }

    /// Returns the launch arguments that are not switches.
    ///
    /// The arguments contain the files or links named by the launch, in order.
    /// The program name is not included.
    pub fn args(&self) -> &[String] {
        &self.args
    }

    /// Returns the value of the `--name` switch.
    ///
    /// Returns an empty string when the switch has no value and `None` when the
    /// switch was not present.
    pub fn switch(&self, name: &str) -> Option<&str> {
        // Chromium lowercases switch names on Windows, lookups included
        #[cfg(target_os = "windows")]
        let name = &name.to_ascii_lowercase();

        self.switches.get(name).map(String::as_str)
    }

    /// The directory the launch was started in, which the relative paths
    /// among its arguments are relative to.
    pub fn working_dir(&self) -> Option<&Path> {
        self.working_dir.as_deref()
    }
}

/// What [`App::on_second_instance`] stores. The browser process handler passes
/// its handle to each call.
pub(crate) type SecondInstanceHandler = Arc<dyn Fn(&SecondInstance, &AppHandle) + Send + Sync>;

/// Describes where the frontend comes from
pub(crate) enum Source {
    Url(String),
    Path(PathBuf),
}

/// Customizes browser-process startup behavior.
///
/// Register via App::delegate to customize browser-process startup
/// without replacing Kurogane's built-in runtime.
///
/// Delegates are invoked in registration order.
pub trait ClientAppBrowserDelegate: Send + Sync {
    /// Invoked before Chromium processes command-line arguments.
    ///
    /// Prefer App::chromium_flag for simple flag configuration.
    /// This hook exists as a lower-level escape hatch.
    fn on_before_command_line_processing(&self, _command_line: &mut CommandLine) {}

    /// Invoked after the browser process has initialized its request context.
    ///
    /// At this point global browser-process initialization has completed and browser creation may begin.
    fn on_context_initialized(&self) {}
}

/// Customizes render-process behavior.
///
/// Register via App::renderer_delegate to observe or extend renderer-side lifecycle events.
///
/// Delegates are invoked in registration order. Depending on the callback,
/// Kurogane may perform built-in renderer processing before or after
/// delegate dispatch. Delegate implementations should not rely on a
/// specific ordering unless documented for a particular callback.
pub trait ClientAppRendererDelegate: Send + Sync {
    /// Invoked once after WebKit initialization.
    ///
    /// Typically used to register V8 extensions and renderer-global state.
    fn on_web_kit_initialized(&self) {}

    /// Invoked when a renderer-side browser instance is created.
    fn on_browser_created(
        &self,
        _browser: Option<&Browser>,
        _extra_info: Option<&DictionaryValue>,
    ) {
    }

    /// Invoked before a renderer-side browser instance is destroyed.
    fn on_browser_destroyed(&self, _browser: Option<&Browser>) {}

    /// Invoked when a JavaScript execution context is created.
    ///
    /// Kurogane's built-in IPC bridge has already been installed when this callback is dispatched.
    fn on_context_created(
        &self,
        _browser: Option<&Browser>,
        _frame: Option<&Frame>,
        _context: Option<&V8Context>,
    ) {
    }

    /// Invoked when a JavaScript execution context is released.
    fn on_context_released(
        &self,
        _browser: Option<&Browser>,
        _frame: Option<&Frame>,
        _context: Option<&V8Context>,
    ) {
    }

    /// Invoked when an uncaught JavaScript exception occurs.
    fn on_uncaught_exception(
        &self,
        _browser: Option<&Browser>,
        _frame: Option<&Frame>,
        _context: Option<&V8Context>,
        _exception: Option<&V8Exception>,
        _stack_trace: Option<&V8StackTrace>,
    ) {
    }

    /// Invoked when the focused DOM node changes.
    fn on_focused_node_changed(
        &self,
        _browser: Option<&Browser>,
        _frame: Option<&Frame>,
        _node: Option<&Domnode>,
    ) {
    }

    /// Invoked when a process message is received from another CEF process.
    ///
    /// Returning a non-zero value marks the message as handled and prevents
    /// subsequent delegates and Kurogane's default processing from running.
    fn on_process_message_received(
        &self,
        _browser: Option<&Browser>,
        _frame: Option<&Frame>,
        _source_process: ProcessId,
        _message: Option<&ProcessMessage>,
    ) -> i32 {
        0
    }

    /// Supplies a renderer-side load handler.
    ///
    /// Delegates are consulted in registration order. The first delegate returning Some(LoadHandler) wins.
    fn load_handler(&self) -> Option<LoadHandler> {
        None
    }
}

/// Public application builder.
///
/// Configures how the first browser instance starts.
///
/// # Processes
///
/// Chromium's helper processes (renderer, GPU, utility) run this same binary
/// again, with a `--type=` argument. In a helper, the call that starts the
/// runtime ([`App::run`], [`App::run_or_exit`], [`App::start`],
/// [`App::build`] or [`App::start_embedded`]) becomes the helper and never
/// returns, so code before it runs once per process. Keep side effects
/// (files, output, sockets, spawned processes) out of that code. Guard them
/// with [`is_browser_process`](crate::is_browser_process), or move them into
/// [`ClientAppBrowserDelegate::on_context_initialized`] which only the
/// browser process calls.
pub struct App {
    source: Source,
    sync_handlers: HashMap<String, SyncHandler>,
    async_handlers: HashMap<String, AsyncHandler>,
    stream_handlers: HashMap<String, StreamFactory>,

    acl: crate::acl::CommandAcl,

    start_window: Option<WindowOptions>,
    window_identity: WindowIdentity,
    profile_id: Option<String>,
    sandbox_mode: SandboxMode,
    persist_session_cookies: bool,
    gpu_mode: GpuMode,
    credential_storage: CredentialStorage,
    chromium_flags: Vec<ChromiumFlag>,
    scheduler: Option<PumpScheduler>,
    on_second_instance: Option<SecondInstanceHandler>,
    hooks: Hooks,
    delegates: Vec<Arc<dyn ClientAppBrowserDelegate>>,
    renderer_delegates: Vec<Arc<dyn ClientAppRendererDelegate>>,
    scheme_handlers: Vec<CustomScheme>,

    /// Builder misuse, reported together by `build()` before anything starts.
    problems: Vec<ConfigError>,

    /// A filesystem configuration its builder rejected, reported by `build()`.
    filesystem_error: Option<FsConfigError>,
}

impl App {
    /// Create an app from a local directory (default entrypoint)
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self::with_source(Source::Path(path.into()))
    }

    /// Start from an explicit URL (escape hatch for power users)
    pub fn url(url: impl Into<String>) -> Self {
        Self::with_source(Source::Url(url.into()))
    }

    fn with_source(source: Source) -> Self {
        let acl = crate::acl::CommandAcl::new(resolver::app_origin(&source));
        Self {
            source,
            sync_handlers: HashMap::new(),
            async_handlers: HashMap::new(),
            stream_handlers: HashMap::new(),
            acl,

            start_window: None,
            window_identity: WindowIdentity::default(),
            profile_id: None,
            sandbox_mode: SandboxMode::default(),
            persist_session_cookies: true,
            gpu_mode: GpuMode::Auto,
            credential_storage: CredentialStorage::System,
            chromium_flags: Vec::new(),
            scheduler: None,
            on_second_instance: None,
            hooks: Hooks::default(),
            delegates: Vec::new(),
            renderer_delegates: Vec::new(),
            scheme_handlers: Vec::new(),
            problems: Vec::new(),
            filesystem_error: None,
        }
    }

    /// Records a second registration of `name`; `build()` reports it.
    fn guard_unique_name(&mut self, name: &str) {
        if self.sync_handlers.contains_key(name)
            || self.async_handlers.contains_key(name)
            || self.stream_handlers.contains_key(name)
        {
            self.problems
                .push(ConfigError::DuplicateHandler(name.to_owned()));
        }
    }

    /// Fails with every recorded configuration problem.
    fn check_configuration(&mut self) -> Result<(), RuntimeError> {
        if !self.problems.is_empty() {
            return Err(RuntimeError::InvalidConfiguration(std::mem::take(
                &mut self.problems,
            )));
        }

        match self.filesystem_error.take() {
            Some(error) => Err(RuntimeError::InvalidFilesystem(error)),
            None => Ok(()),
        }
    }

    /// Restricts the command or stream `name` to the given origins.
    ///
    /// An origin is `scheme://host[:port]`, as `location.origin` reports it
    /// (`app://app` for the bundled frontend, or the development server's origin);
    /// parse one with [`Origin::parse`]. Calls for the same name accumulate and
    /// the list replaces the default below, name the application's own origin
    /// too if its pages call `name`.
    ///
    /// A name without a rule is reachable from the application's own origin
    /// only: `app://app` for [`App::new`], the start URL's origin for
    /// [`App::url`]. A page of any other origin, whether it arrived in a popup,
    /// an iframe or a window navigated away, reaches only the names a rule gives
    /// it. An opaque document (`about:blank`, `data:`, a sandboxed frame)
    /// reaches only names made public with [`App::permit_all`]. A refused
    /// invocation is rejected with [`ErrorCode::Acl`](crate::ErrorCode::Acl); a
    /// refused stream open fails the stream.
    ///
    /// Naming a capability command such as `fs.read_file` (granted through
    /// [`Filesystem`](crate::capability::Filesystem)) or the opaque origin is
    /// a configuration error, reported by [`App::build`].
    pub fn permit(
        mut self,
        name: impl Into<String>,
        origins: impl IntoIterator<Item = Origin>,
    ) -> Self {
        if let Err(problem) = self.acl.allow(name, origins) {
            self.problems.push(problem);
        }
        self
    }

    /// Makes the command or stream `name` callable from any origin, the
    /// opaque origin included.
    ///
    /// Naming a capability command is a configuration error, reported by
    /// [`App::build`].
    pub fn permit_all(mut self, name: impl Into<String>) -> Self {
        if let Err(problem) = self.acl.allow_all(name) {
            self.problems.push(problem);
        }
        self
    }

    /// Restricts subscriptions to the event `name` to the given origins.
    /// Calls for the same event accumulate. Without a rule, an event is
    /// subscribable from the application's own origin only, as a command is
    /// (see [`App::permit`]).
    ///
    /// A refused subscription is removed and reported to the `onError` of
    /// `kurogane.on(name, callback, onError)` with code `-4`. Naming the
    /// opaque origin is a configuration error, reported by [`App::build`].
    pub fn permit_event(
        mut self,
        name: impl Into<String>,
        origins: impl IntoIterator<Item = Origin>,
    ) -> Self {
        if let Err(problem) = self.acl.allow_event(name, origins) {
            self.problems.push(problem);
        }
        self
    }

    /// Makes the event `name` subscribable from any origin, the opaque origin
    /// included.
    pub fn permit_event_all(mut self, name: impl Into<String>) -> Self {
        self.acl.allow_event_all(name);
        self
    }

    /// Switches to deny-by-default: a name without a rule is refused to every
    /// origin, the application's own included. Only commands, streams and
    /// events with a configured rule ([`App::permit`], [`App::permit_all`],
    /// [`App::permit_event`], [`App::permit_event_all`]) stay reachable, only
    /// from their permitted origins. Capability commands stay authorized by
    /// their grants.
    pub fn deny_unlisted(mut self) -> Self {
        self.acl.deny_unlisted();
        self
    }

    /// Installs the filesystem capability: the `fs.*` commands, each call
    /// authorized by filesystem grants for the invoking frame's origin.
    ///
    /// Grants are the only authorization for these commands; the ACL never
    /// gates them, `permit` cannot name them and an origin without a grant
    /// is rejected with [`ErrorCode::Capability`](crate::ErrorCode::Capability).
    /// Without this call there are no `fs.*` commands at all.
    ///
    /// A handler or ACL rule already using an `fs.*` name (including a second
    /// call to this method) is a configuration error, reported by [`App::build`].
    ///
    /// Takes the builder rather than a built
    /// [`Filesystem`](crate::capability::Filesystem), because the roots are
    /// opened here in the browser process only. Helper processes run this
    /// same code but never serve `fs.*`, and a directory handle a helper
    /// opened before entering Chromium's sandbox would stay usable inside it.
    /// A configuration the builder rejects is reported by [`App::build`] as
    /// [`RuntimeError::InvalidFilesystem`].
    pub fn filesystem(self, fs: FilesystemBuilder) -> Self {
        self.install_filesystem(fs, crate::runtime::is_browser_process())
    }

    fn install_filesystem(mut self, fs: FilesystemBuilder, browser_process: bool) -> Self {
        if !browser_process {
            return self;
        }

        let fs = match fs.build() {
            Ok(fs) => fs,
            Err(error) => {
                self.filesystem_error = Some(error);
                return self;
            }
        };

        fs.prepare_matching();
        for (command, handler) in crate::capability::commands::handlers(fs) {
            let name = command.name();
            self.guard_unique_name(name);
            if let Err(problem) = self.acl.capability(name) {
                self.problems.push(problem);
            }
            self.async_handlers.insert(name.to_owned(), handler);
        }
        self
    }

    /// Register a browser lifecycle delegate.
    pub fn delegate<D: ClientAppBrowserDelegate + 'static>(mut self, delegate: D) -> Self {
        self.delegates.push(Arc::new(delegate));
        self
    }

    /// Register a render process lifecycle delegate.
    pub fn renderer_delegate<D: ClientAppRendererDelegate + 'static>(
        mut self,
        delegate: D,
    ) -> Self {
        self.renderer_delegates.push(Arc::new(delegate));
        self
    }

    /// Register a handler for a custom URL scheme.
    ///
    /// The scheme becomes loadable by the frontend (e.g. for a scheme named
    /// `data`, URLs `data://...`). The handler is invoked for every request on
    /// the scheme regardless of host.
    ///
    /// The built-in `app` scheme (used to serve bundled assets) is reserved
    /// and cannot be overridden.
    ///
    /// An invalid or reserved name, or one registered twice, is a
    /// configuration error, reported by [`App::build`].
    pub fn register_scheme<H: SchemeHandler + 'static>(
        mut self,
        name: impl Into<String>,
        handler: H,
    ) -> Self {
        let name = name.into();
        if let Err(reason) = validate_scheme_name(&name) {
            self.problems
                .push(ConfigError::InvalidScheme { name, reason });
        } else if self.scheme_handlers.iter().any(|s| s.name == name) {
            self.problems.push(ConfigError::DuplicateScheme(name));
        } else {
            self.scheme_handlers.push(CustomScheme {
                name,
                handler: Arc::new(handler),
            });
        }
        self
    }

    /// Supply a scheduler callback for pump timing.
    ///
    /// When CEF determines it needs work done, it will call this closure
    /// with a PumpRequest indicating how urgently. The integrator is
    /// responsible for calling AppInstance::pump accordingly;
    /// [`PumpRequest::deadline`] gives the instant to pump at.
    ///
    /// CEF may call the scheduler from any thread. It enables CEF's external
    /// message pump, which returns after each pump. Use it with [`App::start`]
    /// and [`App::start_embedded`]; [`App::run`] does not accept one.
    pub fn scheduler<F>(mut self, f: F) -> Self
    where
        F: Fn(PumpRequest) + Send + Sync + 'static,
    {
        self.scheduler = Some(Arc::new(f));
        self
    }

    /// Registers a synchronous JSON command handler.
    ///
    /// The closure receives the deserialized request and a reference to the
    /// shared runtime handle. Use the handle to broadcast events, query what
    /// is open or end the application.
    ///
    /// Ignore it with _ when not needed.
    ///
    /// A name that is already registered is a configuration error, reported
    /// by [`App::build`].
    ///
    /// # Threads
    ///
    /// The closure runs on the UI thread, which also runs every window and
    /// receives every message from the page: they all wait until it returns,
    /// and CEF says not to block this thread. For slow work use
    /// [`App::async_command`] and resolve its responder from another thread.
    pub fn command<Req, Res, F>(mut self, name: impl Into<String>, f: F) -> Self
    where
        Req: serde::de::DeserializeOwned + Send + 'static,
        Res: serde::Serialize + Send + 'static,
        F: Fn(Req, &AppHandle) -> Result<Res, IpcError> + Send + Sync + 'static,
    {
        let name = name.into();
        self.guard_unique_name(&name);
        self.sync_handlers.insert(
            name,
            Box::new(move |data: &[u8], app: &AppHandle, _ctx| {
                let req: Req = if data.is_empty() {
                    serde_json::from_value(Value::Null)
                } else {
                    serde_json::from_slice(data)
                }
                .map_err(IpcError::from)?;
                let res = f(req, app)?;
                serde_json::to_vec(&res).map_err(IpcError::from)
            }),
        );
        self
    }

    /// Registers an asynchronous JSON command handler.
    ///
    /// The closure receives the deserialized request, a typed responder to
    /// send the response later and the shared runtime handle.
    ///
    /// A name that is already registered is a configuration error, reported
    /// by [`App::build`].
    ///
    /// # Threads
    ///
    /// The closure runs on the UI thread, as for [`App::command`], so it should
    /// return promptly; the [`Responder`] may be resolved from any thread.
    pub fn async_command<Req, Res, F>(mut self, name: impl Into<String>, f: F) -> Self
    where
        Req: serde::de::DeserializeOwned + Send + 'static,
        Res: serde::Serialize + Send + 'static,
        F: Fn(Req, Responder<Res>, &AppHandle) + Send + Sync + 'static,
    {
        let name = name.into();
        self.guard_unique_name(&name);
        self.async_handlers.insert(
            name,
            Box::new(
                move |data: &[u8], responder: BinaryResponder, app: &AppHandle, _ctx| {
                    let req: Req = match if data.is_empty() {
                        serde_json::from_value(Value::Null)
                    } else {
                        serde_json::from_slice(data)
                    } {
                        Ok(r) => r,
                        Err(e) => {
                            responder.resolve(Err(IpcError::from(e)));
                            return;
                        }
                    };
                    let responder =
                        responder.map(|res: Res| serde_json::to_vec(&res).map_err(IpcError::from));
                    f(req, responder, app)
                },
            ),
        );
        self
    }

    /// Registers a synchronous binary command handler.
    ///
    /// The closure receives the raw payload bytes and the shared runtime handle.
    ///
    /// A name that is already registered is a configuration error, reported
    /// by [`App::build`].
    ///
    /// # Threads
    ///
    /// As for [`App::command`]; for slow work use [`App::async_binary_command`].
    pub fn binary_command<F>(mut self, name: impl Into<String>, f: F) -> Self
    where
        F: Fn(&[u8], &AppHandle) -> Result<Vec<u8>, IpcError> + Send + Sync + 'static,
    {
        let name = name.into();
        self.guard_unique_name(&name);
        self.sync_handlers.insert(
            name,
            Box::new(move |data: &[u8], app: &AppHandle, _ctx| f(data, app)),
        );
        self
    }

    /// Registers an asynchronous binary command handler.
    ///
    /// The closure receives the payload bytes (owned), a BinaryResponder to
    /// send the response later and the shared runtime handle.
    ///
    /// A name that is already registered is a configuration error, reported
    /// by [`App::build`].
    ///
    /// # Threads
    ///
    /// As for [`App::async_command`].
    pub fn async_binary_command<F>(mut self, name: impl Into<String>, f: F) -> Self
    where
        F: Fn(Vec<u8>, BinaryResponder, &AppHandle) + Send + Sync + 'static,
    {
        let name = name.into();
        self.guard_unique_name(&name);
        self.async_handlers.insert(
            name,
            Box::new(
                move |data: &[u8], responder: BinaryResponder, app: &AppHandle, _ctx| {
                    f(data.to_vec(), responder, app)
                },
            ),
        );
        self
    }

    /// Registers a stream handler whose factory does not need AppHandle.
    ///
    /// The factory is called for each stream a page opens under `name` that
    /// the ACL lets through, so each stream has a handler, and mutable state,
    /// of its own. [`StreamHandler`](crate::StreamHandler) describes a
    /// stream's life: the handler accepts or refuses the open, then sends.
    ///
    /// A name that is already registered is a configuration error, reported
    /// by [`App::build`].
    ///
    /// # Threads
    ///
    /// The factory and every [`StreamHandler`](crate::StreamHandler)
    /// callback run on the UI thread, as for [`App::command`], so they should
    /// return promptly. A [`StreamResponder`](crate::StreamResponder)
    /// may be used from any thread.
    pub fn stream<F, H>(self, name: impl Into<String>, factory: F) -> Self
    where
        F: Fn() -> H + Send + Sync + 'static,
        H: crate::ipc::StreamHandler + 'static,
    {
        self.stream_h(name, move |_: &AppHandle| factory())
    }

    /// Registers a stream handler whose factory receives &AppHandle.
    ///
    /// Like [`App::stream`], but the factory receives a reference to the
    /// shared runtime handle, useful for broadcasting events or querying
    /// runtime state from within stream lifecycle callbacks.
    ///
    /// A name that is already registered is a configuration error, reported
    /// by [`App::build`].
    ///
    /// # Threads
    ///
    /// As for [`App::stream`].
    pub fn stream_h<F, H>(mut self, name: impl Into<String>, factory: F) -> Self
    where
        F: Fn(&AppHandle) -> H + Send + Sync + 'static,
        H: crate::ipc::StreamHandler + 'static,
    {
        let name = name.into();
        self.guard_unique_name(&name);
        self.stream_handlers.insert(
            name,
            Box::new(move |app: &AppHandle| Box::new(factory(app))),
        );
        self
    }

    /// How the start window opens: the name the application knows it by,
    /// its title, size and place, the size it cannot be made smaller than,
    /// and its state (see [`WindowOptions`]).
    ///
    /// Without it the start window takes its page's title, 800 by 600
    /// centred on the primary display. An application started
    /// with [`App::start_embedded`] has no start window, and options given
    /// to it are [`ConfigError::WindowWhenEmbedded`]; options no window can
    /// have are [`ConfigError::InvalidWindowOptions`]. A later call replaces
    /// an earlier one.
    ///
    /// ```no_run
    /// # use kurogane::{App, WindowOptions, WindowState};
    /// App::new("dist")
    ///     .window(WindowOptions::new().title("Notes").size(1100, 720).state(WindowState::Maximized))
    ///     .run_or_exit();
    /// ```
    pub fn window(mut self, options: WindowOptions) -> Self {
        self.start_window = Some(options);
        self
    }

    /// The class of every window Kurogane opens on Linux: WM_CLASS under
    /// X11, the app_id under Wayland. The desktop and the window manager
    /// tell an application's windows by it: a launcher's icon attaches to
    /// them when its desktop entry names the class (`StartupWMClass` under
    /// X11; under Wayland the entry's file name is the app_id), and the
    /// compositor's rules for the application match it. `kurogane bundle`
    /// writes `[app].identifier` from `kurogane.toml` as the AppImage's
    /// `StartupWMClass`: give this the same identifier.
    ///
    /// It names the start window, [`AppInstance::create_window`]'s windows,
    /// popups and DevTools alike; not a browser embedded in the
    /// application's own window, whose window is the application's. Without
    /// it CEF names the windows. Other platforms have no window class and
    /// ignore it. An empty class, or one with a control character, is
    /// [`ConfigError::InvalidWindowClass`]. A later call replaces an earlier
    /// one.
    ///
    /// ```no_run
    /// # use kurogane::App;
    /// App::new("dist").window_class("com.example.notes").run_or_exit();
    /// ```
    pub fn window_class(mut self, class: impl Into<String>) -> Self {
        self.window_identity.class = Some(class.into());
        self
    }

    /// The icon of every window Kurogane opens, a PNG.
    ///
    /// On Windows it is the window's icon in its title bar, the taskbar and
    /// the window switcher; under X11 the icon the window manager shows.
    /// A square image of 256 by 256 is plenty: the system scales it to each
    /// place. Under Wayland the desktop takes a window's icon from the
    /// desktop entry its class names ([`App::window_class`]), and on macOS
    /// windows have none: the Dock shows the application's own icon.
    ///
    /// It is the icon of the start window,
    /// [`AppInstance::create_window`]'s windows, popups and DevTools alike;
    /// not of a browser embedded in the application's own window. Without
    /// it the windows keep the system's default. Bytes that are not a PNG
    /// are [`ConfigError::InvalidWindowIcon`]. A later call replaces an
    /// earlier one.
    ///
    /// ```no_run
    /// # use kurogane::App;
    /// let icon = std::fs::read("icon.png").expect("the icon");
    /// App::new("dist").window_icon(icon).run_or_exit();
    /// ```
    pub fn window_icon(mut self, png: impl Into<Cow<'static, [u8]>>) -> Self {
        self.window_identity.icon = Some(png.into());
        self
    }

    /// Names the application's profile: its cookies, storage and caches.
    ///
    /// Defaults to the executable's name. CEF runs one instance per profile:
    /// launching the application while it runs brings the running instance to
    /// the front (see [`App::on_second_instance`]). Debug builds use a profile
    /// of their own, named with a `-dev` suffix.
    pub fn profile_id(mut self, id: impl Into<String>) -> Self {
        self.profile_id = Some(id.into());
        self
    }

    /// Runs `f` in the running application whenever it is launched again.
    ///
    /// CEF runs one instance per profile ([`App::profile_id`]). A launch that
    /// finds the application running hands it its arguments and exits with
    /// status 0, and the running application's windows come to the front.
    /// `f` then receives the launch, for example to open a file or a link it
    /// names.
    ///
    /// Runs on the UI thread. On macOS, opening the application's bundle while
    /// it runs activates it without a second launch, so `f` runs only for
    /// launches that start a process, such as running the executable directly.
    ///
    /// A later call replaces an earlier one.
    pub fn on_second_instance<F>(mut self, f: F) -> Self
    where
        F: Fn(&SecondInstance, &AppHandle) + Send + Sync + 'static,
    {
        self.on_second_instance = Some(Arc::new(f));
        self
    }

    /// Decides what happens when a page asks for a window of its own:
    /// `window.open`, a `target=_blank` link, a form that targets a new
    /// window, a link clicked with Ctrl (Cmd on macOS), the middle button or
    /// Shift. An allowed modifier click opens in a new application window,
    /// never in Chromium's tabbed browser window.
    ///
    /// Without a hook, or when it answers [`NewWindowDecision::Default`],
    /// Kurogane decides: a page of the application's own origin opens in an
    /// application window, an `http` or `https` link the user clicked opens
    /// in the system's default browser, and anything else is refused. That
    /// keeps a website the application never chose out of its windows, and
    /// a script alone never starts another program.
    ///
    /// The hook can widen that, for example
    /// [`NewWindowDecision::Allow`] for a sign-in page that has to run
    /// inside the application, or narrow it, with
    /// [`NewWindowDecision::Deny`] or [`NewWindowDecision::OpenExternal`].
    /// [`NewWindowDecision::OpenExternal`] opens only an `http` or `https`
    /// link the user clicked and refuses anything else. Compare origins, not
    /// strings: `https://trusted.example.evil.net` starts with
    /// `https://trusted.example`.
    ///
    /// Runs on the UI thread, before the window exists, so it must not
    /// block. A hook that panics refuses the window. A later call replaces an
    /// earlier one.
    ///
    /// ```no_run
    /// # use kurogane::{App, NewWindowDecision, Origin};
    /// let sign_in = Origin::parse("https://accounts.example.com").unwrap();
    /// App::new("./dist")
    ///     .on_new_window(move |request, _| {
    ///         if request.origin() == &sign_in {
    ///             NewWindowDecision::Allow
    ///         } else {
    ///             NewWindowDecision::Default
    ///         }
    ///     })
    ///     .run_or_exit();
    /// ```
    pub fn on_new_window<F>(mut self, f: F) -> Self
    where
        F: Fn(&NewWindowRequest, &AppHandle) -> NewWindowDecision + Send + Sync + 'static,
    {
        self.hooks.new_window = Some(Box::new(f));
        self
    }

    /// Decides where a page may take the window it is in: a link, a
    /// `location` assignment, a form, and every redirect on the way.
    ///
    /// A window shows only what was let into it: the application's own
    /// origin, the origins the application loaded there itself (the start
    /// page, [`AppInstance::create_window`](crate::AppInstance::create_window),
    /// [`BrowserHandle::navigate`](crate::BrowserHandle::navigate), and the
    /// redirects they lead to), the origin
    /// [`on_new_window`](App::on_new_window) opened a popup to, and the
    /// origins this hook allowed into it before. Without a hook, or when it
    /// answers [`NavigationDecision::Default`], a navigation to one of those
    /// proceeds; to any other origin, an `http` or `https` link the user
    /// clicked opens in the system's default browser and anything else is
    /// refused, the window staying on its page. The application's own loads,
    /// going back and forward, and frames inside a page never reach the
    /// hook.
    ///
    /// [`NavigationDecision::Allow`] loads the page and lets its origin into
    /// that window from then on, for example a sign-in provider the page
    /// sends the user to. [`NavigationDecision::Deny`] and
    /// [`NavigationDecision::OpenExternal`] narrow Kurogane's answer.
    /// Compare origins, not strings.
    ///
    /// Runs on the UI thread before the navigation starts, so it must not
    /// block. A hook that panics refuses the navigation. A later call
    /// replaces an earlier one.
    ///
    /// ```no_run
    /// # use kurogane::{App, NavigationDecision, Origin};
    /// let sign_in = Origin::parse("https://accounts.example.com").unwrap();
    /// App::url("https://app.example.com")
    ///     .on_navigation(move |navigation, _| {
    ///         if navigation.origin() == &sign_in {
    ///             NavigationDecision::Allow
    ///         } else {
    ///             NavigationDecision::Default
    ///         }
    ///     })
    ///     .run_or_exit();
    /// ```
    pub fn on_navigation<F>(mut self, f: F) -> Self
    where
        F: Fn(&NavigationRequest, &AppHandle) -> NavigationDecision + Send + Sync + 'static,
    {
        self.hooks.navigation = Some(Box::new(f));
        self
    }

    /// Sees each key the user presses in a window of the application's
    /// before the page and Chromium's own shortcuts do, and decides where it
    /// goes.
    ///
    /// The hook is asked about key presses only (the key going down, and
    /// its repeats while held), never about the release or the character
    /// it types. [`KeyDecision::Default`] lets the key go on in Chromium's
    /// own order: the shortcuts Chromium reserves (opening, closing and
    /// switching tabs and windows, such as Ctrl+T and Ctrl+W) take it before
    /// the page sees it, every other shortcut only after the page, if the
    /// page does not prevent the key's default. [`KeyDecision::Consume`]
    /// takes the key from everyone else: Chromium's shortcuts do not run
    /// (Ctrl+W does not close the window, Ctrl+R does not reload) and the
    /// page sees neither the key, its character nor its release.
    /// [`KeyDecision::PageFirst`] gives the page the key first even for a
    /// reserved shortcut, which then runs only if the page lets it through;
    /// it is for shortcuts, and lets the page keep one from running (see
    /// its documentation). Compare keys with
    /// [`Modifiers::primary`](crate::Modifiers::primary) to match Ctrl on
    /// Windows and Linux and Cmd on macOS in one test, and mind
    /// [`KeyPress::in_editable_field`](crate::KeyPress::in_editable_field)
    /// so as not to take keys the user is typing.
    ///
    /// Runs on the UI thread for every key press, so it must be quick. A
    /// hook that panics lets the key through. DevTools' windows are not
    /// asked about. A later call replaces an earlier one.
    ///
    /// ```no_run
    /// # use kurogane::{App, Key, KeyDecision};
    /// App::new("./dist")
    ///     .on_key(|key, _| {
    ///         // Ctrl+W, Cmd+W on macOS, does not close the window
    ///         if key.key() == Key::Char('W') && key.modifiers().primary() {
    ///             KeyDecision::Consume
    ///         } else {
    ///             KeyDecision::Default
    ///         }
    ///     })
    ///     .run_or_exit();
    /// ```
    pub fn on_key<F>(mut self, f: F) -> Self
    where
        F: Fn(&KeyPress, &AppHandle) -> KeyDecision + Send + Sync + 'static,
    {
        self.hooks.key = Some(Box::new(f));
        self
    }

    /// Decides whether one of Chromium's commands runs: one of the page-local
    /// commands Kurogane lets run, from a key shortcut or the context menu
    /// (reload, find, print, zoom, editing, closing the window, DevTools).
    ///
    /// Kurogane refuses every other command first (new windows, new tabs,
    /// history, bookmarks: Chromium's browser UI), and the hook is never
    /// asked about those: it can refuse a command, never allow one.
    /// [`ChromeCommand`](crate::ChromeCommand) folds Chromium's commands by
    /// what the user asked for, so refusing
    /// [`ChromeCommand::DevTools`](crate::ChromeCommand::DevTools) refuses
    /// the shortcuts and the context menu's Inspect alike. A key the
    /// [`on_key`](App::on_key) hook consumed never becomes a command.
    ///
    /// Runs on the UI thread. A hook that panics refuses the command.
    /// DevTools' own commands are not asked about. A later call replaces an
    /// earlier one.
    ///
    /// ```no_run
    /// # use kurogane::{App, ChromeCommand, CommandDecision};
    /// App::new("./dist")
    ///     .on_chrome_command(|request, _| match request.command() {
    ///         // No DevTools and no reload in the shipped application
    ///         ChromeCommand::DevTools | ChromeCommand::Reload => CommandDecision::Refuse,
    ///         _ => CommandDecision::Default,
    ///     })
    ///     .run_or_exit();
    /// ```
    pub fn on_chrome_command<F>(mut self, f: F) -> Self
    where
        F: Fn(&ChromeCommandRequest, &AppHandle) -> CommandDecision + Send + Sync + 'static,
    {
        self.hooks.chrome_command = Some(Box::new(f));
        self
    }

    /// Decides where a file a page downloads is saved: a link the server
    /// answers with an attachment, a link with a `download` attribute, a
    /// `blob:` or `data:` export.
    ///
    /// Without a hook, or when it answers [`DownloadDecision::Default`], the
    /// user is asked with the system's Save As dialog, the suggested name
    /// filled in, and nothing is saved if they cancel: no page writes to
    /// the disk unless the user picked the place. A window shows one
    /// dialog at a time. Chromium's own behaviour, saving silently into the
    /// Downloads folder with nothing on screen, is never used.
    /// [`DownloadDecision::SaveTo`] saves at an absolute path the
    /// application chose, without asking, for example its own exports into
    /// a folder of its own; [`DownloadDecision::Deny`] saves nothing.
    ///
    /// Every download a page starts reaches the hook, several at once
    /// included: Chromium's prompt for multiple downloads never shows.
    ///
    /// Runs on the UI thread before the download starts, so it must not
    /// block. A hook that panics refuses the download. Downloads from
    /// DevTools are not asked about and always ask the user. A later call
    /// replaces an earlier one.
    ///
    /// ```no_run
    /// # use kurogane::{App, DownloadDecision, Origin};
    /// let exports = std::env::temp_dir().join("my-app-exports");
    /// let own = Origin::parse("app://app").unwrap();
    /// App::new("./dist")
    ///     .on_download(move |download, _| {
    ///         if download.origin() == &own {
    ///             DownloadDecision::SaveTo(exports.join(download.suggested_name()))
    ///         } else {
    ///             DownloadDecision::Deny
    ///         }
    ///     })
    ///     .run_or_exit();
    /// ```
    pub fn on_download<F>(mut self, f: F) -> Self
    where
        F: Fn(&DownloadRequest, &AppHandle) -> DownloadDecision + Send + Sync + 'static,
    {
        self.hooks.download = Some(Box::new(f));
        self
    }

    /// Decides what a page may use only with consent: a camera, a
    /// microphone, the screen, the location, notifications, reading the
    /// clipboard and the rest of [`Permission`](crate::Permission).
    ///
    /// Without a hook, or when it answers [`PermissionDecision::Default`],
    /// the request is denied: no page gets a device or a permission the
    /// application did not allow. Chromium's own prompt never shows, in a
    /// window or an embedded browser. [`PermissionDecision::Allow`] grants
    /// everything the request asks for and [`PermissionDecision::Deny`]
    /// none of it: a page asking for a camera and a microphone together
    /// gets both or neither.
    ///
    /// To ask the user first, the hook takes a
    /// [`PermissionResponder`](crate::PermissionResponder) with
    /// [`PermissionRequest::responder`] and answers
    /// [`PermissionDecision::Later`]; the page waits until the responder
    /// allows or denies, from any thread. A responder dropped unanswered
    /// denies.
    ///
    /// Allowing [`Permission::ScreenVideo`](crate::Permission::ScreenVideo)
    /// lets the page record the whole screen: there is no picker.
    ///
    /// Chromium remembers its answer to a web site (http, https), granted or
    /// denied, in the profile, so the site's later requests get it without
    /// reaching the hook; [`AppHandle::forget_permissions`] makes them ask
    /// again. A camera, a microphone and the screen are never remembered.
    /// For the application's own pages nothing is remembered, so a grant
    /// that must still hold after the request does not: such a page is told
    /// yes for notifications and the location, but cannot show a
    /// notification or read the location.
    ///
    /// Runs on the UI thread, so it must not block. A hook that panics
    /// denies. Requests from DevTools are not asked about and are denied. A
    /// later call replaces an earlier one.
    ///
    /// ```no_run
    /// # use kurogane::{App, Origin, Permission, PermissionDecision};
    /// let own = Origin::parse("app://app").unwrap();
    /// App::new("./dist")
    ///     .on_permission(move |request, _| {
    ///         let devices = request
    ///             .permissions()
    ///             .iter()
    ///             .all(|kind| matches!(kind, Permission::Camera | Permission::Microphone));
    ///         if request.origin() == &own && devices {
    ///             PermissionDecision::Allow
    ///         } else {
    ///             PermissionDecision::Deny
    ///         }
    ///     })
    ///     .run_or_exit();
    /// ```
    pub fn on_permission<F>(mut self, f: F) -> Self
    where
        F: Fn(&PermissionRequest, &AppHandle) -> PermissionDecision + Send + Sync + 'static,
    {
        self.hooks.permission = Some(Box::new(f));
        self
    }

    /// Edits the menu a right-click opens.
    ///
    /// Kurogane builds the same menu in a window and in an embedded
    /// browser, never Chromium's own: in a text field the editing items
    /// (Undo, Redo, Cut, Copy, Paste, Paste as plain text, Select all),
    /// under Chromium's spelling suggestions on a misspelled word; Copy on
    /// a selection; nothing elsewhere; and Inspect last in a debug build.
    /// The hook gets that [`ContextMenu`](crate::ContextMenu), with what
    /// was right-clicked ([`ContextMenu::target`](crate::ContextMenu::target):
    /// a link, an image, a selection, a text field), and may add, remove or
    /// reorder its items: Kurogane's own
    /// ([`StandardItem`](crate::StandardItem)), the application's
    /// ([`MenuItem::new`](crate::MenuItem::new), whose choice goes to
    /// [`on_context_menu_command`](App::on_context_menu_command)),
    /// separators and submenus. A menu left empty does not show.
    ///
    /// A standard item whose command
    /// [`on_chrome_command`](App::on_chrome_command) refuses is left out,
    /// and asked about again when chosen. Separators at an edge or next to
    /// another, and submenus left empty, are not shown. A page that draws
    /// its own menu calls `preventDefault()` on the `contextmenu` event, and
    /// no menu of Kurogane's opens.
    ///
    /// Runs on the UI thread as the menu opens, so it must not block. A
    /// hook that panics leaves Kurogane's menu. DevTools' menus are not
    /// asked about. A later call replaces an earlier one.
    ///
    /// ```no_run
    /// # use kurogane::{App, MenuItem};
    /// App::new("./dist")
    ///     .on_context_menu(|menu, _| {
    ///         if menu.target().link_url().is_some() {
    ///             menu.push(MenuItem::new("copy-link", "Copy link"));
    ///         }
    ///     })
    ///     .on_context_menu_command(|command, _| {
    ///         if command.id() == "copy-link" {
    ///             println!("copy {:?}", command.target().link_url());
    ///         }
    ///     })
    ///     .run_or_exit();
    /// ```
    pub fn on_context_menu<F>(mut self, f: F) -> Self
    where
        F: Fn(&mut ContextMenu, &AppHandle) + Send + Sync + 'static,
    {
        self.hooks.context_menu = Some(Box::new(f));
        self
    }

    /// Runs an item of the application's that the user chose from a
    /// context menu ([`MenuItem::new`](crate::MenuItem::new) in
    /// [`on_context_menu`](App::on_context_menu)), with what the menu was
    /// opened on. An item chosen after the document the menu was opened on
    /// went away, a page that navigated while its menu stayed open, runs
    /// nothing.
    ///
    /// Runs on the UI thread, so it must not block. A hook that panics is
    /// logged. A later call replaces an earlier one.
    pub fn on_context_menu_command<F>(mut self, f: F) -> Self
    where
        F: Fn(&ContextMenuCommand, &AppHandle) + Send + Sync + 'static,
    {
        self.hooks.context_menu_command = Some(Box::new(f));
        self
    }

    /// Hears each window of the application's close, with which it is and
    /// where it was, to open it there again next time.
    ///
    /// Asked for the start window, every [`AppInstance::create_window`]
    /// window and every window a page opened as the application's, once
    /// each, however it closes: the user, the page, the application, Ctrl+C.
    /// Not for popups, DevTools or a browser embedded in the application's
    /// own window. [`WindowClosing::name`](crate::WindowClosing::name) is
    /// the name the window's options gave it, free for a new window from
    /// here on, and
    /// [`WindowClosing::placement`](crate::WindowClosing::placement) where it
    /// was and how it showed: given back to
    /// [`WindowOptions::placement`] it opens the window as it was, on a
    /// display that is still there.
    ///
    /// Runs on the UI thread as the window closes, before the application's
    /// last window ends it: saving a small settings file here is fine,
    /// anything slow is not. A hook that panics is logged, and the window
    /// closes as it would. A later call replaces an earlier one.
    ///
    /// ```no_run
    /// # use kurogane::{App, WindowOptions, WindowPlacement};
    /// # fn load(name: &str) -> Option<WindowPlacement> { None }
    /// # fn save(name: &str, placement: WindowPlacement) {}
    /// let mut main = WindowOptions::new().name("main").size(1100, 720);
    /// if let Some(placement) = load("main") {
    ///     main = main.placement(placement);
    /// }
    /// App::new("dist")
    ///     .window(main)
    ///     .on_window_closing(|closing, _| {
    ///         if let Some(name) = closing.name() {
    ///             save(name, closing.placement());
    ///         }
    ///     })
    ///     .run_or_exit();
    /// ```
    pub fn on_window_closing<F>(mut self, f: F) -> Self
    where
        F: Fn(&WindowClosing, &AppHandle) + Send + Sync + 'static,
    {
        self.hooks.window_closing = Some(Box::new(f));
        self
    }

    /// Sets the Chromium process sandbox policy.
    ///
    /// Defaults to [`SandboxMode::Disabled`].
    ///
    /// [`SandboxMode::Chromium`] is checked before CEF starts and fails with a
    /// [`RuntimeError`] when this platform or machine cannot enforce it. See
    /// [`SandboxMode::Chromium`] for the per-platform requirements.
    pub fn sandbox_mode(mut self, mode: SandboxMode) -> Self {
        self.sandbox_mode = mode;
        self
    }

    pub fn persist_session_cookies(mut self, value: bool) -> Self {
        self.persist_session_cookies = value;
        self
    }

    /// Override GPU backend selection.
    pub fn gpu_mode(mut self, mode: GpuMode) -> Self {
        self.gpu_mode = mode;
        self
    }

    /// Override how cookies and saved passwords are protected at rest.
    ///
    /// Defaults to the platform credential store. `CredentialStorage::Basic`
    /// trades encryption for a fixed built-in key, which keeps unsigned builds
    /// and keyring-less hosts from prompting on every run.
    pub fn credential_storage(mut self, storage: CredentialStorage) -> Self {
        self.credential_storage = storage;
        self
    }

    /// Add a Chromium flag with no value.
    ///
    /// The name is a Chromium switch name, with or without its leading `--`
    /// or `-` (or `/` on Windows); on Windows it is case-insensitive, as
    /// Chromium treats it. The flag overrides the runtime's own setting of
    /// the same switch; the feature lists, which only add (see
    /// [`App::chromium_flag_with_value`]), are left as they are.
    pub fn chromium_flag(mut self, name: impl Into<String>) -> Self {
        self.chromium_flags.push(ChromiumFlag::Present(name.into()));
        self
    }

    /// Add a Chromium flag with a value.
    ///
    /// The name is read as [`App::chromium_flag`] reads it. The last value
    /// given for a switch wins, over the runtime's own value too, except
    /// for `disable-features` and `enable-features`: their value adds its
    /// features to the list, after CEF's own, the ones the application was
    /// launched with and those given before, each feature once.
    ///
    /// ```no_run
    /// # use kurogane::App;
    /// // CEF's own disabled features stay disabled
    /// App::new("dist")
    ///     .chromium_flag_with_value("disable-features", "Translate,MediaRouter")
    ///     .run_or_exit();
    /// ```
    pub fn chromium_flag_with_value(
        mut self,
        name: impl Into<String>,
        value: impl Into<String>,
    ) -> Self {
        self.chromium_flags
            .push(ChromiumFlag::WithValue(name.into(), value.into()));
        self
    }

    /// Initialize CEF and return an AppInstance.
    ///
    /// In a Chromium helper process this never returns; see
    /// [Processes](App#processes).
    ///
    /// # Errors
    ///
    /// [`RuntimeError::InvalidConfiguration`] lists every builder problem
    /// before anything starts; the other variants report startup failures.
    pub fn build(self) -> Result<AppInstance, RuntimeError> {
        self.launch(RuntimeMode::Views)
    }

    /// Starts the runtime in embedded mode.
    ///
    /// # Errors
    ///
    /// As [`App::build`].
    pub fn start_embedded(self) -> Result<AppInstance, RuntimeError> {
        self.launch(RuntimeMode::Embedded)
    }

    /// Checks the configuration, then starts the runtime in `mode`.
    fn launch(mut self, mode: RuntimeMode) -> Result<AppInstance, RuntimeError> {
        if let Some(options) = &self.start_window {
            if mode == RuntimeMode::Embedded {
                self.problems.push(ConfigError::WindowWhenEmbedded);
            } else if let Some(problem) = options.problem() {
                self.problems
                    .push(ConfigError::InvalidWindowOptions(problem));
            }
        }
        if let Some(class) = &self.window_identity.class {
            if class.is_empty() {
                self.problems.push(ConfigError::InvalidWindowClass(
                    "a window class is not empty",
                ));
            } else if class.chars().any(char::is_control) {
                self.problems.push(ConfigError::InvalidWindowClass(
                    "a window class has no control character",
                ));
            }
        }
        // A PNG begins with its signature; CEF decodes the rest with each
        // window
        if let Some(png) = &self.window_identity.icon
            && !png.starts_with(b"\x89PNG\r\n\x1a\n")
        {
            self.problems.push(ConfigError::InvalidWindowIcon);
        }
        self.check_configuration()?;

        let Self {
            source,
            sync_handlers,
            async_handlers,
            stream_handlers,
            acl,
            start_window,
            window_identity,
            profile_id,
            sandbox_mode,
            persist_session_cookies,
            gpu_mode,
            credential_storage,
            chromium_flags,
            scheduler,
            on_second_instance,
            hooks,
            delegates,
            renderer_delegates,
            scheme_handlers,
            ..
        } = self;

        let rpc = RequestResponseSubsystem::new(sync_handlers, async_handlers);
        let event = EventSubsystem::new();
        let stream = StreamSubsystem::new(stream_handlers);
        let router = IpcRouter::new(rpc, event, stream, acl);

        let ResolvedFrontend {
            asset_root,
            start_url,
        } = resolver::resolve_for_process(&source)?;

        let spec = RuntimeSpec {
            mode,
            sandbox_mode,
            start_url,
            start_window: start_window.unwrap_or_default(),
            window_identity,
            asset_root,
            profile_id,
            persist_session_cookies,
            gpu_mode,
            credential_storage,
            chromium_flags,
            scheduler,
            on_second_instance,
            hooks: Arc::new(hooks),
            delegates,
            renderer_delegates,
            scheme_handlers,
        };

        crate::runtime::start(spec, router)
    }

    /// Start the application and run the message loop.
    ///
    /// # Errors
    ///
    /// As [`App::build`]. Returns [`ConfigError::SchedulerWithRunLoop`] when a
    /// [`App::scheduler`] is configured, since [`App::run`] owns the message loop.
    pub fn run(mut self) -> Result<(), RuntimeError> {
        if self.scheduler.is_some() {
            self.problems.push(ConfigError::SchedulerWithRunLoop);
        }
        self.build()?.run()
    }

    /// Initialize the application without entering a message loop.
    pub fn start(self) -> Result<AppInstance, RuntimeError> {
        self.build()
    }

    /// Run the application and terminate the process on failure, after
    /// printing the error and each of its causes.
    /// Intended for binaries. Libraries embedding the runtime should use run() instead.
    pub fn run_or_exit(self) {
        if let Err(e) = self.run() {
            use std::io::Write as _;
            // Report startup failure directly to stderr
            let mut stderr = std::io::stderr().lock();
            let _ = writeln!(stderr, "\nApplication failed to start:\n{e}\n");
            let mut cause = std::error::Error::source(&e);
            while let Some(error) = cause {
                let _ = writeln!(stderr, "Caused by: {error}");
                cause = error.source();
            }
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn json_noop(_: Value, _: &AppHandle) -> Result<Value, IpcError> {
        Ok(Value::Null)
    }

    fn binary_noop(_: &[u8], _: &AppHandle) -> Result<Vec<u8>, IpcError> {
        Ok(vec![])
    }

    fn async_noop(_: Value, r: Responder<Value>, _: &AppHandle) {
        r.resolve(Ok(Value::Null));
    }

    /// What a handler receives with a message from `app://app`.
    fn context() -> crate::ipc::IpcContext {
        crate::ipc::IpcContext {
            browser_id: crate::browser_registry::BrowserId::new(1),
            frame: crate::ipc::FrameId::new("test-frame"),
            origin: origin("app://app"),
            url_origin: origin("app://app"),
        }
    }

    /// Whether `call` ended the application of the handle it was given. With
    /// nothing open, a shutdown ends it at once.
    fn ends(call: impl FnOnce(&AppHandle)) -> bool {
        let handle = AppHandle::detached();
        call(&handle);
        handle.should_shutdown()
    }

    fn ignored() -> BinaryResponder {
        BinaryResponder::new(Box::new(|_| {}))
    }

    #[test]
    fn handlers_use_the_handle_they_are_called_with() {
        let app = App::new("./dist")
            .command("json", |_: Value, handle: &AppHandle| {
                handle.shutdown();
                Ok(Value::Null)
            })
            .binary_command("bytes", |_: &[u8], handle: &AppHandle| {
                handle.shutdown();
                Ok(Vec::new())
            })
            .async_command(
                "json-later",
                |_: Value, _: Responder<Value>, handle: &AppHandle| handle.shutdown(),
            )
            .async_binary_command(
                "bytes-later",
                |_: Vec<u8>, _: BinaryResponder, handle: &AppHandle| handle.shutdown(),
            )
            .stream_h("stream", |handle: &AppHandle| {
                handle.shutdown();
                NoopStream
            })
            .on_second_instance(|_: &SecondInstance, handle: &AppHandle| handle.shutdown())
            .on_new_window(|_: &NewWindowRequest, handle: &AppHandle| {
                handle.shutdown();
                NewWindowDecision::Default
            })
            .on_navigation(|_: &NavigationRequest, handle: &AppHandle| {
                handle.shutdown();
                NavigationDecision::Default
            })
            .on_key(|_: &KeyPress, handle: &AppHandle| {
                handle.shutdown();
                KeyDecision::Default
            })
            .on_chrome_command(|_: &ChromeCommandRequest, handle: &AppHandle| {
                handle.shutdown();
                CommandDecision::Default
            })
            .on_download(|_: &DownloadRequest, handle: &AppHandle| {
                handle.shutdown();
                DownloadDecision::Default
            });

        assert!(ends(|h| {
            app.sync_handlers["json"](b"", h, context()).unwrap();
        }));
        assert!(ends(|h| {
            app.sync_handlers["bytes"](b"", h, context()).unwrap();
        }));
        assert!(ends(|h| app.async_handlers["json-later"](
            b"",
            ignored(),
            h,
            context()
        )));
        assert!(ends(|h| app.async_handlers["bytes-later"](
            b"",
            ignored(),
            h,
            context()
        )));
        assert!(ends(|h| drop(app.stream_handlers["stream"](h))));
        let launch = SecondInstance {
            args: Vec::new(),
            switches: HashMap::new(),
            working_dir: None,
        };
        let hook = app.on_second_instance.as_ref().expect("registered");
        assert!(ends(|h| hook(&launch, h)));
        let request = NewWindowRequest::new("about:blank".into(), "app://app/", false);
        let hook = app.hooks.new_window.as_ref().expect("registered");
        assert!(ends(|h| {
            hook(&request, h);
        }));
        let navigation =
            NavigationRequest::new("app://app/b.html".into(), "app://app/", false, false);
        let hook = app.hooks.navigation.as_ref().expect("registered");
        assert!(ends(|h| {
            hook(&navigation, h);
        }));
        let press = KeyPress::new(0x57, 0, u16::from(b'w'), false, None);
        let hook = app.hooks.key.as_ref().expect("registered");
        assert!(ends(|h| {
            hook(&press, h);
        }));
        let command = ChromeCommandRequest::new(crate::ChromeCommand::Reload, None);
        let hook = app.hooks.chrome_command.as_ref().expect("registered");
        assert!(ends(|h| {
            hook(&command, h);
        }));
        let download = DownloadRequest::new(
            "app://app/a.txt".into(),
            "app://app/",
            "a.txt".into(),
            "text/plain".into(),
            None,
        );
        let hook = app.hooks.download.as_ref().expect("registered");
        assert!(ends(|h| {
            hook(&download, h);
        }));
    }

    #[test]
    fn a_second_instance_reads_switches_as_chromium_parsed_them() {
        let launch = SecondInstance {
            args: vec!["notes.txt".to_owned()],
            switches: HashMap::from([
                ("new-window".to_owned(), String::new()),
                ("theme".to_owned(), "dark".to_owned()),
            ]),
            working_dir: None,
        };

        assert_eq!(launch.switch("new-window"), Some(""));
        assert_eq!(launch.switch("theme"), Some("dark"));
        assert_eq!(launch.switch("notes.txt"), None);

        // Chromium stores them lowercased on Windows, and looks them up so
        #[cfg(target_os = "windows")]
        assert_eq!(launch.switch("New-Window"), Some(""));
    }

    struct NoopStream;

    impl crate::ipc::StreamHandler for NoopStream {
        fn on_chunk(&mut self, _: &[u8], _: &crate::ipc::StreamResponder) -> Result<(), IpcError> {
            Ok(())
        }
    }

    struct NoScheme;

    impl SchemeHandler for NoScheme {
        fn create(
            &self,
            _: Option<&mut Browser>,
            _: Option<&mut Frame>,
            _: Option<&mut Request>,
        ) -> Option<ResourceHandler> {
            None
        }
    }

    fn duplicate(name: &str) -> Vec<ConfigError> {
        vec![ConfigError::DuplicateHandler(name.to_owned())]
    }

    fn origin(text: &str) -> Origin {
        Origin::parse(text).unwrap()
    }

    #[test]
    fn every_handler_kind_shares_one_namespace() {
        let cases = [
            App::new("./dist")
                .command("x", json_noop)
                .command("x", json_noop),
            App::new("./dist")
                .binary_command("x", binary_noop)
                .binary_command("x", binary_noop),
            App::new("./dist")
                .command("x", json_noop)
                .binary_command("x", binary_noop),
            App::new("./dist")
                .binary_command("x", binary_noop)
                .command("x", json_noop),
            App::new("./dist")
                .command("x", json_noop)
                .async_command("x", async_noop),
            App::new("./dist")
                .async_command("x", async_noop)
                .command("x", json_noop),
            App::new("./dist")
                .async_command("x", async_noop)
                .binary_command("x", binary_noop),
            App::new("./dist")
                .stream("x", || NoopStream)
                .stream("x", || NoopStream),
            App::new("./dist")
                .stream("x", || NoopStream)
                .command("x", json_noop),
            App::new("./dist")
                .command("x", json_noop)
                .stream("x", || NoopStream),
            App::new("./dist")
                .stream("x", || NoopStream)
                .async_command("x", async_noop),
        ];
        for app in cases {
            assert_eq!(app.problems, duplicate("x"));
        }
    }

    #[test]
    fn build_reports_problems_before_starting_anything() {
        let result = App::new("./dist")
            .command("x", json_noop)
            .command("x", json_noop)
            .build();
        match result {
            Err(RuntimeError::InvalidConfiguration(problems)) => {
                assert_eq!(problems, duplicate("x"))
            }
            Err(other) => panic!("expected a configuration error, got: {other}"),
            Ok(_) => panic!("a misconfigured app must not start"),
        }
    }

    #[test]
    fn window_settings_no_window_can_have_are_refused_before_anything_starts() {
        let problems = |app: App, embedded: bool| {
            let result = if embedded {
                app.start_embedded()
            } else {
                app.build()
            };
            match result {
                Err(RuntimeError::InvalidConfiguration(problems)) => problems,
                Err(other) => panic!("expected a configuration error, got: {other}"),
                Ok(_) => panic!("a misconfigured app must not start"),
            }
        };
        let window = || crate::WindowOptions::new().title("x");
        assert_eq!(
            problems(App::new("./dist").window(window()), true),
            vec![ConfigError::WindowWhenEmbedded]
        );
        assert!(matches!(
            problems(
                App::new("./dist").window(window().size(800, 600).min_size(900, 1)),
                false
            )[..],
            [ConfigError::InvalidWindowOptions(_)]
        ));
        assert!(matches!(
            problems(App::new("./dist").window(window().name("")), false)[..],
            [ConfigError::InvalidWindowOptions(_)]
        ));
        for class in ["", "notes\n"] {
            assert!(
                matches!(
                    problems(App::new("./dist").window_class(class), false)[..],
                    [ConfigError::InvalidWindowClass(_)]
                ),
                "{class:?}"
            );
        }
        for icon in [&b""[..], b"GIF89a", b"\x89PNG\r\n"] {
            assert_eq!(
                problems(App::new("./dist").window_icon(icon), false),
                vec![ConfigError::InvalidWindowIcon],
                "{icon:?}"
            );
        }
    }

    #[test]
    fn run_refuses_a_scheduler() {
        let result = App::new("./dist").scheduler(|_| {}).run();
        match result {
            Err(RuntimeError::InvalidConfiguration(problems)) => {
                assert_eq!(problems, vec![ConfigError::SchedulerWithRunLoop])
            }
            Err(other) => panic!("expected a configuration error, got: {other}"),
            Ok(()) => panic!("App::run must refuse a scheduler"),
        }
    }

    #[test]
    fn a_pump_request_is_due_after_its_delay() {
        let now = Instant::now();
        let delay = Duration::from_millis(40);

        assert_eq!(PumpRequest::Now.deadline(now), now);
        assert_eq!(PumpRequest::After(delay).deadline(now), now + delay);
        // Past what an Instant holds: now, since pumping early is safe
        assert_eq!(PumpRequest::After(Duration::MAX).deadline(now), now);

        // Copy and Eq: a host can keep a request and compare it
        let request = PumpRequest::After(delay);
        let kept = request;
        assert_eq!(request, kept);
    }

    #[test]
    fn scheme_names_are_validated() {
        let app = App::new("./dist")
            .register_scheme("app", NoScheme)
            .register_scheme("1data", NoScheme)
            .register_scheme("data", NoScheme)
            .register_scheme("data", NoScheme);
        assert!(matches!(
            app.problems.as_slice(),
            [
                ConfigError::InvalidScheme { .. },
                ConfigError::InvalidScheme { .. },
                ConfigError::DuplicateScheme(name),
            ] if name == "data"
        ));
        assert_eq!(app.scheme_handlers.len(), 1);
    }

    fn empty_filesystem() -> FilesystemBuilder {
        crate::capability::Filesystem::builder()
    }

    /// A configuration the builder rejects, whatever the machine.
    fn rejected_filesystem() -> FilesystemBuilder {
        let mut builder = crate::capability::Filesystem::builder();
        let scope = builder.scope("data", |_| {});
        builder.grant(Origin::OPAQUE, scope, crate::capability::FsAccess::READ);
        builder
    }

    #[test]
    fn a_rejected_filesystem_is_reported_before_starting_anything() {
        let result = App::new("./dist").filesystem(rejected_filesystem()).build();

        assert!(matches!(
            result,
            Err(RuntimeError::InvalidFilesystem(FsConfigError::OpaqueOrigin))
        ));
    }

    #[test]
    fn helper_processes_open_no_filesystem_roots() {
        // The builder would fail if it were built, so a clean helper means
        // nothing was opened; nor is anything registered to serve
        let helper = App::new("./dist").install_filesystem(rejected_filesystem(), false);

        assert!(helper.filesystem_error.is_none());
        assert!(
            !helper
                .async_handlers
                .keys()
                .any(|name| name.starts_with("fs."))
        );
    }

    #[test]
    fn filesystem_registers_every_fs_command() {
        let app = App::new("./dist").filesystem(empty_filesystem());
        for command in crate::capability::policy::FsCommand::ALL {
            assert!(
                app.async_handlers.contains_key(command.name()),
                "{}",
                command.name()
            );
        }
        let bare = App::new("./dist");
        assert!(
            !bare
                .async_handlers
                .keys()
                .any(|name| name.starts_with("fs."))
        );
    }

    #[test]
    fn fs_names_clash_with_other_handlers_in_either_order() {
        let after = App::new("./dist")
            .filesystem(empty_filesystem())
            .command("fs.read_file", json_noop);
        assert_eq!(after.problems, duplicate("fs.read_file"));
        let before = App::new("./dist")
            .binary_command("fs.size", binary_noop)
            .filesystem(empty_filesystem());
        assert_eq!(before.problems, duplicate("fs.size"));
    }

    #[test]
    fn acl_rules_cannot_name_capability_commands() {
        let permitted_after = App::new("./dist")
            .filesystem(empty_filesystem())
            .permit("fs.read_file", [origin("app://app")]);
        assert_eq!(
            permitted_after.problems,
            vec![ConfigError::CapabilityCommand("fs.read_file".to_owned())]
        );
        let permitted_before = App::new("./dist")
            .permit_all("fs.write_file")
            .filesystem(empty_filesystem());
        assert_eq!(
            permitted_before.problems,
            vec![ConfigError::CapabilityCommand("fs.write_file".to_owned())]
        );
    }

    #[test]
    fn the_opaque_origin_cannot_be_permitted() {
        let app = App::new("./dist")
            .permit("ping", [Origin::OPAQUE])
            .permit_event("tick", [Origin::OPAQUE]);
        assert_eq!(
            app.problems,
            vec![
                ConfigError::OpaqueOrigin("ping".to_owned()),
                ConfigError::OpaqueOrigin("tick".to_owned()),
            ]
        );
    }

    #[test]
    fn event_rules_are_recorded_separately_from_commands() {
        let app = App::new("./dist")
            .permit_event("tick", [origin("app://app")])
            .permit_event_all("public")
            .deny_unlisted();
        assert!(app.problems.is_empty());
        assert!(app.acl.allows_event("tick", &origin("app://app")));
        assert!(
            !app.acl
                .allows_event("tick", &origin("https://evil.example"))
        );
        assert!(
            app.acl
                .allows_event("public", &origin("https://evil.example"))
        );
        assert!(!app.acl.allows("tick", &origin("app://app")));
    }
}
