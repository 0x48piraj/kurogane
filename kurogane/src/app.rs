//! High level application bootstrap API.
//!
//! This is the public developer entrypoint built on top of Runtime.
//! This helps in the abstraction of asset resolution, environment overrides and command registration.

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

    profile_id: Option<String>,
    sandbox_mode: SandboxMode,
    persist_session_cookies: bool,
    gpu_mode: GpuMode,
    credential_storage: CredentialStorage,
    chromium_flags: Vec<ChromiumFlag>,
    scheduler: Option<PumpScheduler>,
    on_second_instance: Option<SecondInstanceHandler>,
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
        Self {
            source,
            sync_handlers: HashMap::new(),
            async_handlers: HashMap::new(),
            stream_handlers: HashMap::new(),
            acl: crate::acl::CommandAcl::new(),

            profile_id: None,
            sandbox_mode: SandboxMode::default(),
            persist_session_cookies: true,
            gpu_mode: GpuMode::Auto,
            credential_storage: CredentialStorage::System,
            chromium_flags: Vec::new(),
            scheduler: None,
            on_second_instance: None,
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
    /// parse one with [`Origin::parse`]. Calls for the same name accumulate.
    ///
    /// Until the first `permit`, `permit_all` or `deny_unlisted`, everything is
    /// reachable from every origin, exactly as before. A refused invocation is
    /// rejected with [`ErrorCode::Acl`](crate::ErrorCode::Acl); a refused stream
    /// open fails the stream.
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

    /// Makes the command or stream `name` callable from any origin.
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
    /// Calls for the same event accumulate.
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

    /// Makes the event `name` subscribable from any origin.
    pub fn permit_event_all(mut self, name: impl Into<String>) -> Self {
        self.acl.allow_event_all(name);
        self
    }

    /// Switches to deny-by-default. Only commands, streams and events with a
    /// configured rule ([`App::permit`], [`App::permit_all`],
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
    /// of its own. [`StreamHandler`](crate::ipc::StreamHandler) describes a
    /// stream's life: the handler accepts or refuses the open, then sends.
    ///
    /// A name that is already registered is a configuration error, reported
    /// by [`App::build`].
    ///
    /// # Threads
    ///
    /// The factory and every [`StreamHandler`](crate::ipc::StreamHandler)
    /// callback run on the UI thread, as for [`App::command`], so they should
    /// return promptly. A [`StreamResponder`](crate::ipc::StreamResponder)
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
    /// the same switch.
    pub fn chromium_flag(mut self, name: impl Into<String>) -> Self {
        self.chromium_flags.push(ChromiumFlag::Present(name.into()));
        self
    }

    /// Add a Chromium flag with a value.
    ///
    /// The name is read as [`App::chromium_flag`] reads it. The last value
    /// given for a switch wins, over the runtime's own value too.
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
        self.check_configuration()?;

        let Self {
            source,
            sync_handlers,
            async_handlers,
            stream_handlers,
            acl,
            profile_id,
            sandbox_mode,
            persist_session_cookies,
            gpu_mode,
            credential_storage,
            chromium_flags,
            scheduler,
            on_second_instance,
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
            asset_root,
            profile_id,
            persist_session_cookies,
            gpu_mode,
            credential_storage,
            chromium_flags,
            scheduler,
            on_second_instance,
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
            eprintln!("\nApplication failed to start:\n{e}\n");
            let mut cause = std::error::Error::source(&e);
            while let Some(error) = cause {
                eprintln!("Caused by: {error}");
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
            .on_second_instance(|_: &SecondInstance, handle: &AppHandle| handle.shutdown());

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
        fn on_chunk(&mut self, _: &[u8], _: &crate::ipc::StreamResponder) -> Result<(), String> {
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
