//! High level application bootstrap API.
//!
//! This is the public developer entrypoint built on top of Runtime.
//! This helps in the abstraction of asset resolution, environment overrides and command registration.

use std::path::PathBuf;
use std::time::Duration;
use std::sync::Arc;
use serde_json::Value;
use std::collections::HashMap;
use cef::*;
use crate::app::resolver::ResolvedFrontend;
use crate::ipc::{
    AppCell, IpcRouter, RequestResponseSubsystem, EventSubsystem, StreamSubsystem, StreamFactory,
    Responder, BinaryResponder, SyncHandler, AsyncHandler, IpcContext, IpcError,
};
use crate::runtime::{RuntimeBootstrap, AppHandle, AppInstance};
use crate::error::{ConfigError, RuntimeError};
use crate::spec::{RuntimeSpec, RuntimeMode, SandboxMode};
use crate::scheme::{CustomScheme, SchemeHandler, validate_scheme_name};
use crate::chromium_flags::ChromiumFlag;
use crate::credentials::CredentialStorage;
use crate::gpu::GpuMode;
use crate::capability::Filesystem;
use crate::acl::Origin;

mod resolver;

/// A request from CEF indicating when it next needs to be serviced.
///
/// Passed to the scheduler closure supplied via App::scheduler.
#[derive(Debug, Clone)]
pub enum PumpRequest {
    /// CEF needs work immediately.
    Now,
    /// CEF needs work after the given delay.
    After(Duration),
}

/// Callback type for pump scheduling.
///
/// CEF calls this whenever it wants AppInstance::pump to be called.
/// The integrator decides how to honour the request via a winit proxy, a glib timeout, a Tokio task, or anything else.
pub type PumpScheduler = Arc<dyn Fn(PumpRequest) + Send + Sync>;

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
/// Delegates are invoked in registration order. The first delegate returning a client from Self::default_client wins.
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

    /// Supplies a custom default Client implementation.
    ///
    /// The returned client will be used when Kurogane creates browser
    /// instances unless another delegate registered earlier has already supplied one.
    ///
    /// Returning None defers to subsequent delegates or Kurogane's built-in client implementation.
    fn default_client(&self) -> Option<Client> {
        None
    }
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
    cell: AppCell,
    resolver: Option<crate::ipc::handle_cell::AppCellResolver>,

    profile_id: Option<String>,
    sandbox_mode: SandboxMode,
    persist_session_cookies: bool,
    gpu_mode: GpuMode,
    credential_storage: CredentialStorage,
    chromium_flags: Vec<ChromiumFlag>,
    scheduler: Option<PumpScheduler>,
    delegates: Vec<Arc<dyn ClientAppBrowserDelegate>>,
    renderer_delegates: Vec<Arc<dyn ClientAppRendererDelegate>>,
    scheme_handlers: Vec<CustomScheme>,

    /// Builder misuse, reported together by `build()` before anything starts.
    problems: Vec<ConfigError>,
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
        let (cell, resolver) = AppCell::new();
        Self {
            source,
            sync_handlers: HashMap::new(),
            async_handlers: HashMap::new(),
            stream_handlers: HashMap::new(),
            acl: crate::acl::CommandAcl::new(),
            cell,
            resolver: Some(resolver),

            profile_id: None,
            sandbox_mode: SandboxMode::default(),
            persist_session_cookies: true,
            gpu_mode: GpuMode::Auto,
            credential_storage: CredentialStorage::System,
            chromium_flags: Vec::new(),
            scheduler: None,
            delegates: Vec::new(),
            renderer_delegates: Vec::new(),
            scheme_handlers: Vec::new(),
            problems: Vec::new(),
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
        if self.problems.is_empty() {
            Ok(())
        } else {
            Err(RuntimeError::InvalidConfiguration(std::mem::take(
                &mut self.problems,
            )))
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
    /// [`Filesystem`]) or the opaque origin is a configuration error, reported
    /// by [`App::build`].
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
    pub fn filesystem(mut self, fs: Filesystem) -> Self {
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
    /// responsible for calling AppInstance::pump accordingly.
    ///
    /// Only meaningful when using App::start_embedded / App::start
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
    /// shared runtime handle. Use the handle to broadcast events, spawn
    /// background work, or query runtime state.
    ///
    /// Ignore it with _ when not needed.
    ///
    /// A name that is already registered is a configuration error, reported
    /// by [`App::build`].
    pub fn command<Req, Res, F>(mut self, name: impl Into<String>, f: F) -> Self
    where
        Req: serde::de::DeserializeOwned + Send + 'static,
        Res: serde::Serialize + Send + 'static,
        F: Fn(Req, &AppHandle) -> Result<Res, IpcError> + Send + Sync + 'static,
    {
        let name = name.into();
        self.guard_unique_name(&name);
        let cell = self.cell.clone();
        self.sync_handlers.insert(
            name,
            Box::new(move |data: &[u8], _ctx: IpcContext| {
                let req: Req = if data.is_empty() {
                    serde_json::from_value(Value::Null)
                } else {
                    serde_json::from_slice(data)
                }
                .map_err(IpcError::from)?;
                let res = f(req, cell.get())?;
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
    pub fn async_command<Req, Res, F>(mut self, name: impl Into<String>, f: F) -> Self
    where
        Req: serde::de::DeserializeOwned + Send + 'static,
        Res: serde::Serialize + Send + 'static,
        F: Fn(Req, Responder<Res>, &AppHandle) + Send + Sync + 'static,
    {
        let name = name.into();
        self.guard_unique_name(&name);
        let cell = self.cell.clone();
        self.async_handlers.insert(
            name,
            Box::new(
                move |data: &[u8], responder: BinaryResponder, _ctx: IpcContext| {
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
                    f(req, responder, cell.get())
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
    pub fn binary_command<F>(mut self, name: impl Into<String>, f: F) -> Self
    where
        F: Fn(&[u8], &AppHandle) -> Result<Vec<u8>, IpcError> + Send + Sync + 'static,
    {
        let name = name.into();
        self.guard_unique_name(&name);
        let cell = self.cell.clone();
        self.sync_handlers.insert(
            name,
            Box::new(move |data: &[u8], _ctx: IpcContext| f(data, cell.get())),
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
    pub fn async_binary_command<F>(mut self, name: impl Into<String>, f: F) -> Self
    where
        F: Fn(Vec<u8>, BinaryResponder, &AppHandle) + Send + Sync + 'static,
    {
        let name = name.into();
        self.guard_unique_name(&name);
        let cell = self.cell.clone();
        self.async_handlers.insert(
            name,
            Box::new(
                move |data: &[u8], responder: BinaryResponder, _ctx: IpcContext| {
                    f(data.to_vec(), responder, cell.get())
                },
            ),
        );
        self
    }

    /// Registers a stream handler whose factory does not need AppHandle.
    ///
    /// Stream handlers process data chunks sent from the renderer. The factory
    /// closure is called once per stream open to create a dedicated handler
    /// instance, giving each stream its own mutable state.
    ///
    /// A name that is already registered is a configuration error, reported
    /// by [`App::build`].
    pub fn stream<F, H>(mut self, name: impl Into<String>, factory: F) -> Self
    where
        F: Fn() -> H + Send + Sync + 'static,
        H: crate::ipc::StreamHandler + 'static,
    {
        let name = name.into();
        self.guard_unique_name(&name);
        self.stream_handlers
            .insert(name, Box::new(move || Box::new(factory())));
        self
    }

    /// Registers a stream handler whose factory receives &AppHandle.
    ///
    /// Identical to stream(Self::stream) but the factory receives a
    /// reference to the shared runtime handle, useful for broadcasting events
    /// or querying runtime state from within stream lifecycle callbacks.
    ///
    /// A name that is already registered is a configuration error, reported
    /// by [`App::build`].
    pub fn stream_h<F, H>(mut self, name: impl Into<String>, factory: F) -> Self
    where
        F: Fn(&AppHandle) -> H + Send + Sync + 'static,
        H: crate::ipc::StreamHandler + 'static,
    {
        let name = name.into();
        self.guard_unique_name(&name);
        let cell = self.cell.clone();
        self.stream_handlers.insert(
            name,
            Box::new(move || Box::new(factory(cell.get())) as Box<dyn crate::ipc::StreamHandler>),
        );
        self
    }

    pub fn profile_id(mut self, id: impl Into<String>) -> Self {
        self.profile_id = Some(id.into());
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
    pub fn chromium_flag(mut self, name: impl Into<String>) -> Self {
        self.chromium_flags.push(ChromiumFlag::Present(name.into()));
        self
    }

    /// Add a Chromium flag with a value.
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
    pub fn build(mut self) -> Result<AppInstance, RuntimeError> {
        self.check_configuration()?;
        let resolver = self.resolver.take().expect("build called twice");

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
            delegates,
            renderer_delegates,
            scheme_handlers,
            ..
        } = self;

        let rpc = RequestResponseSubsystem::new(sync_handlers, async_handlers);
        let event = EventSubsystem::new();
        let stream = StreamSubsystem::new(stream_handlers);
        let router = Arc::new(IpcRouter::new(rpc, event, stream, acl));

        let ResolvedFrontend {
            asset_root,
            start_url,
        } = resolver::resolve_for_process(&source)?;

        let spec = RuntimeSpec {
            mode: RuntimeMode::Views,
            sandbox_mode,
            start_url,
            asset_root,
            profile_id,
            persist_session_cookies,
            gpu_mode,
            credential_storage,
            chromium_flags,
            scheduler,
            delegates,
            renderer_delegates,
            scheme_handlers,
        };

        let instance = RuntimeBootstrap::start(spec, router)?;
        // Populated before the message loop starts
        resolver.resolve(instance.handle().clone());
        Ok(instance)
    }

    /// Starts the runtime in embedded mode.
    ///
    /// # Errors
    ///
    /// As [`App::build`].
    pub fn start_embedded(mut self) -> Result<AppInstance, RuntimeError> {
        self.check_configuration()?;
        let resolver = self.resolver.take().expect("start_embedded called twice");

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
            delegates,
            renderer_delegates,
            scheme_handlers,
            ..
        } = self;

        let rpc = RequestResponseSubsystem::new(sync_handlers, async_handlers);
        let event = EventSubsystem::new();
        let stream = StreamSubsystem::new(stream_handlers);
        let router = Arc::new(IpcRouter::new(rpc, event, stream, acl));

        let ResolvedFrontend {
            asset_root,
            start_url,
        } = resolver::resolve_for_process(&source)?;

        let spec = RuntimeSpec {
            mode: RuntimeMode::Embedded,
            sandbox_mode,
            start_url,
            asset_root,
            profile_id,
            persist_session_cookies,
            gpu_mode,
            credential_storage,
            chromium_flags,
            scheduler,
            delegates,
            renderer_delegates,
            scheme_handlers,
        };

        let instance = RuntimeBootstrap::start_embedded(spec, router)?;
        resolver.resolve(instance.handle().clone());
        Ok(instance)
    }

    /// Start the application and run the message loop.
    pub fn run(self) -> Result<(), RuntimeError> {
        self.build()?.run()
    }

    /// Initialize the application without entering a message loop.
    pub fn start(self) -> Result<AppInstance, RuntimeError> {
        self.build()
    }

    /// Run the application and terminate the process on failure.
    /// Intended for binaries. Libraries embedding the runtime should use run() instead.
    pub fn run_or_exit(self) {
        if let Err(e) = self.run() {
            eprintln!("\nApplication failed to start:\n{e}\n");
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

    fn empty_filesystem() -> Filesystem {
        Filesystem::builder()
            .build()
            .expect("an empty configuration always builds")
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
