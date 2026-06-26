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
use crate::ipc::{IpcRouter, RpcSubsystem, BinarySubsystem, EventSubsystem, StreamSubsystem, StreamHandler, IpcResponder, BinaryResponder, SyncRpcHandler, SyncBinaryHandler, AsyncRpcHandler, AsyncBinaryHandler, IpcContext};
use crate::runtime::{RuntimeBootstrap, Runtime};
use crate::error::RuntimeError;
use crate::spec::{RuntimeSpec, RuntimeMode};
use crate::chromium_flags::ChromiumFlag;
use crate::gpu::GpuMode;

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
/// CEF calls this whenever it wants Runtime::pump to be called.
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
    ) {}

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
    ) {}

    /// Invoked when a JavaScript execution context is released.
    fn on_context_released(
        &self,
        _browser: Option<&Browser>,
        _frame: Option<&Frame>,
        _context: Option<&V8Context>,
    ) {}

    /// Invoked when an uncaught JavaScript exception occurs.
    fn on_uncaught_exception(
        &self,
        _browser: Option<&Browser>,
        _frame: Option<&Frame>,
        _context: Option<&V8Context>,
        _exception: Option<&V8Exception>,
        _stack_trace: Option<&V8StackTrace>,
    ) {}

    /// Invoked when the focused DOM node changes.
    fn on_focused_node_changed(
        &self,
        _browser: Option<&Browser>,
        _frame: Option<&Frame>,
        _node: Option<&Domnode>,
    ) {}

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
pub struct App {
    source: Source,
    sync_commands: HashMap<String, SyncRpcHandler>,
    sync_binary_commands: HashMap<String, SyncBinaryHandler>,
    async_commands: HashMap<String, AsyncRpcHandler>,
    async_binary_commands: HashMap<String, AsyncBinaryHandler>,
    stream_handlers: HashMap<String, StreamHandler>,

    profile_id: Option<String>,
    persist_session_cookies: bool,
    gpu_mode: GpuMode,
    chromium_flags: Vec<ChromiumFlag>,
    scheduler: Option<PumpScheduler>,
    delegates: Vec<Arc<dyn ClientAppBrowserDelegate>>,
    renderer_delegates: Vec<Arc<dyn ClientAppRendererDelegate>>,
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
            sync_commands: HashMap::new(),
            sync_binary_commands: HashMap::new(),
            async_commands: HashMap::new(),
            async_binary_commands: HashMap::new(),
            stream_handlers: HashMap::new(),

            profile_id: None,
            persist_session_cookies: true,
            gpu_mode: GpuMode::Auto,
            chromium_flags: Vec::new(),
            scheduler: None,
            delegates: Vec::new(),
            renderer_delegates: Vec::new(),
        }
    }

    /// Register a browser lifecycle delegate.
    pub fn delegate<D: ClientAppBrowserDelegate + 'static>(mut self, delegate: D) -> Self {
        self.delegates.push(Arc::new(delegate));
        self
    }

    /// Register a render process lifecycle delegate.
    pub fn renderer_delegate<D: ClientAppRendererDelegate + 'static>(mut self, delegate: D) -> Self {
        self.renderer_delegates.push(Arc::new(delegate));
        self
    }

    /// Supply a scheduler callback for pump timing.
    ///
    /// When CEF determines it needs work done, it will call this closure
    /// with a PumpRequest indicating how urgently. The integrator is
    /// responsible for calling Runtime::pump accordingly.
    ///
    /// Only meaningful when using App::start_embedded / App::start
    pub fn scheduler<F>(mut self, f: F) -> Self
    where
        F: Fn(PumpRequest) + Send + Sync + 'static,
    {
        self.scheduler = Some(Arc::new(f));
        self
    }

    /// Register a JSON command handler.
    ///
    /// Panics if name has already been registered.
    pub fn command<F>(mut self, name: impl Into<String>, handler: F) -> Self
    where
        F: Fn(Value) -> Result<Value, String> + Send + Sync + 'static,
    {
        let name = name.into();

        if self.sync_commands.contains_key(&name)
            || self.sync_binary_commands.contains_key(&name)
            || self.async_commands.contains_key(&name)
            || self.async_binary_commands.contains_key(&name)
            || self.stream_handlers.contains_key(&name)
        {
            panic!("command '{name}' registered twice");
        }

        let wrapped: SyncRpcHandler = Box::new(move |payload: &str, _ctx: IpcContext| {
            let input: Value = if payload.is_empty() {
                Value::Null
            } else {
                serde_json::from_str(payload)
                    .map_err(|e| format!("invalid JSON payload: {e}"))?
            };

            let output = handler(input)?;

            serde_json::to_string(&output)
                .map_err(|e| format!("JSON serialization error: {e}"))
        });

        self.sync_commands.insert(name, wrapped);
        self
    }

    /// Register a binary command handler.
    ///
    /// Panics if name has already been registered.
    pub fn binary_command<F>(mut self, name: impl Into<String>, handler: F) -> Self
    where
        F: Fn(&[u8]) -> Result<Vec<u8>, String> + Send + Sync + 'static,
    {
        let name = name.into();

        if self.sync_commands.contains_key(&name)
            || self.sync_binary_commands.contains_key(&name)
            || self.async_commands.contains_key(&name)
            || self.async_binary_commands.contains_key(&name)
            || self.stream_handlers.contains_key(&name)
        {
            panic!("command '{name}' registered twice");
        }

        let wrapped: SyncBinaryHandler = Box::new(move |payload: &[u8], _ctx: IpcContext| {
            handler(payload)
        });
        self.sync_binary_commands.insert(name, wrapped);
        self
    }

    /// Register an async JSON command handler.
    ///
    /// The handler receives the parsed JSON value and an IpcResponder.
    /// It should start async work and call responder.resolve() when done.
    ///
    /// Panics if name has already been registered.
    pub fn async_command<F>(mut self, name: impl Into<String>, handler: F) -> Self
    where
        F: Fn(Value, IpcResponder) + Send + Sync + 'static,
    {
        let name = name.into();

        if self.sync_commands.contains_key(&name)
            || self.sync_binary_commands.contains_key(&name)
            || self.async_commands.contains_key(&name)
            || self.async_binary_commands.contains_key(&name)
            || self.stream_handlers.contains_key(&name)
        {
            panic!("command '{name}' registered twice");
        }

        let wrapped: AsyncRpcHandler = Box::new(move |value: Value, responder: IpcResponder, _ctx: IpcContext| {
            handler(value, responder)
        });
        self.async_commands.insert(name, wrapped);
        self
    }

    /// Register an async binary command handler.
    ///
    /// The handler receives owned bytes and a BinaryResponder.
    /// It should start async work and call responder.resolve() when done.
    ///
    /// Panics if name has already been registered.
    pub fn async_binary_command<F>(mut self, name: impl Into<String>, handler: F) -> Self
    where
        F: Fn(Vec<u8>, BinaryResponder) + Send + Sync + 'static,
    {
        let name = name.into();

        if self.sync_commands.contains_key(&name)
            || self.sync_binary_commands.contains_key(&name)
            || self.async_commands.contains_key(&name)
            || self.async_binary_commands.contains_key(&name)
            || self.stream_handlers.contains_key(&name)
        {
            panic!("command '{name}' registered twice");
        }

        let wrapped: AsyncBinaryHandler = Box::new(move |payload: Vec<u8>, responder: BinaryResponder, _ctx: IpcContext| {
            handler(payload, responder)
        });
        self.async_binary_commands.insert(name, wrapped);
        self
    }

    /// Registers a stream handler.
    ///
    /// Stream handlers process data chunks sent from the renderer. Each
    /// invocation receives the stream id, payload chunk, completion flag,
    /// handler name and execution context.
    ///
    /// Panics if a handler with the same name is already registered.
    pub fn stream_handler<F>(mut self, name: impl Into<String>, handler: F) -> Self
    where
        F: Fn(u32, &[u8], bool, &str, IpcContext) -> Result<(), String> + Send + Sync + 'static,
    {
        let name = name.into();

        if self.sync_commands.contains_key(&name)
            || self.sync_binary_commands.contains_key(&name)
            || self.async_commands.contains_key(&name)
            || self.async_binary_commands.contains_key(&name)
            || self.stream_handlers.contains_key(&name)
        {
            panic!("handler '{name}' registered twice");
        }

        let wrapped: StreamHandler = Box::new(handler);
        self.stream_handlers.insert(name, wrapped);
        self
    }

    pub fn profile_id(mut self, id: impl Into<String>) -> Self {
        self.profile_id = Some(id.into());
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

    /// Starts the runtime in embedded mode.
    /// Intended for embedded integrations where the host application owns the
    /// window hierarchy and event loop.
    pub fn start_embedded(self) -> Result<Runtime, RuntimeError> {
        let Self {
            source,
            sync_commands,
            sync_binary_commands,
            async_commands,
            async_binary_commands,
            stream_handlers,
            profile_id,
            persist_session_cookies,
            gpu_mode,
            chromium_flags,
            scheduler,
            delegates,
            renderer_delegates,
        } = self;

        let rpc = RpcSubsystem::new(sync_commands, async_commands);
        let binary = BinarySubsystem::new(sync_binary_commands, async_binary_commands);
        let event = EventSubsystem::new();
        let stream = StreamSubsystem::new(stream_handlers);
        let router = Arc::new(IpcRouter::new(rpc, binary, event, stream));

        let ResolvedFrontend { asset_root, start_url } = resolver::resolve(&source)?;

        let spec = RuntimeSpec {
            mode: RuntimeMode::Embedded,
            start_url,
            asset_root,
            profile_id,
            persist_session_cookies,
            gpu_mode,
            chromium_flags,
            scheduler,
            delegates,
            renderer_delegates,
        };

        RuntimeBootstrap::start_embedded(spec, router)
    }

    /// Start the application and run the message loop.
    ///
    /// Kurogane owns the event loop and blocks until shutdown.
    pub fn run(self) -> Result<(), RuntimeError> {
        let Self {
            source,
            sync_commands,
            sync_binary_commands,
            async_commands,
            async_binary_commands,
            stream_handlers,
            profile_id,
            persist_session_cookies,
            gpu_mode,
            chromium_flags,
            delegates,
            renderer_delegates,
            ..
        } = self;

        let rpc = RpcSubsystem::new(sync_commands, async_commands);
        let binary = BinarySubsystem::new(sync_binary_commands, async_binary_commands);
        let event = EventSubsystem::new();
        let stream = StreamSubsystem::new(stream_handlers);
        let router = Arc::new(IpcRouter::new(rpc, binary, event, stream));

        let ResolvedFrontend { asset_root, start_url } = resolver::resolve(&source)?;

        let spec = RuntimeSpec {
            mode: RuntimeMode::Views,
            start_url,
            asset_root,
            profile_id,
            persist_session_cookies,
            gpu_mode,
            chromium_flags,
            scheduler: None,
            delegates,
            renderer_delegates,
        };

        RuntimeBootstrap::run(spec, router)
    }

    /// Initialize the application without entering a message loop.
    ///
    /// The caller becomes responsible for driving the runtime via Runtime::pump().
    pub fn start(self) -> Result<Runtime, RuntimeError> {
        let Self {
            source,
            sync_commands,
            sync_binary_commands,
            async_commands,
            async_binary_commands,
            stream_handlers,
            profile_id,
            persist_session_cookies,
            gpu_mode,
            chromium_flags,
            scheduler,
            delegates,
            renderer_delegates,
        } = self;

        let rpc = RpcSubsystem::new(sync_commands, async_commands);
        let binary = BinarySubsystem::new(sync_binary_commands, async_binary_commands);
        let event = EventSubsystem::new();
        let stream = StreamSubsystem::new(stream_handlers);
        let router = Arc::new(IpcRouter::new(rpc, binary, event, stream));

        let ResolvedFrontend { asset_root, start_url } = resolver::resolve(&source)?;

        let spec = RuntimeSpec {
            mode: RuntimeMode::Views,
            start_url,
            asset_root,
            profile_id,
            persist_session_cookies,
            gpu_mode,
            chromium_flags,
            scheduler,
            delegates,
            renderer_delegates,
        };

        RuntimeBootstrap::start(spec, router)
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

    fn json_noop(_: Value) -> Result<Value, String> {
        Ok(Value::Null)
    }

    fn binary_noop(_: &[u8]) -> Result<Vec<u8>, String> {
        Ok(vec![])
    }

    #[test]
    #[should_panic(expected = "registered twice")]
    fn duplicate_json_command_panics() {
        App::new("./dist")
            .command("ping", json_noop)
            .command("ping", json_noop);
    }

    #[test]
    #[should_panic(expected = "registered twice")]
    fn duplicate_binary_command_panics() {
        App::new("./dist")
            .binary_command("upload", binary_noop)
            .binary_command("upload", binary_noop);
    }

    #[test]
    #[should_panic(expected = "registered twice")]
    fn json_and_binary_names_cannot_collide() {
        App::new("./dist")
            .command("transfer", json_noop)
            .binary_command("transfer", binary_noop);
    }

    #[test]
    #[should_panic(expected = "registered twice")]
    fn binary_and_json_names_cannot_collide() {
        App::new("./dist")
            .binary_command("transfer", binary_noop)
            .command("transfer", json_noop);
    }
}
