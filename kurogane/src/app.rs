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
use crate::ipc::{AppCell, IpcRouter, RequestResponseSubsystem, EventSubsystem, StreamSubsystem, StreamFactory, IpcResponder, BinaryResponder, SyncHandler, AsyncHandler, IpcContext};
use crate::runtime::{RuntimeBootstrap, AppHandle, AppInstance};
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
    sync_handlers: HashMap<String, SyncHandler>,
    async_handlers: HashMap<String, AsyncHandler>,
    stream_handlers: HashMap<String, StreamFactory>,

    cell: AppCell,
    resolver: Option<crate::ipc::handle_cell::AppCellResolver>,

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
        let (cell, resolver) = AppCell::new();
        Self {
            source,
            sync_handlers: HashMap::new(),
            async_handlers: HashMap::new(),
            stream_handlers: HashMap::new(),
            cell,
            resolver: Some(resolver),

            profile_id: None,
            persist_session_cookies: true,
            gpu_mode: GpuMode::Auto,
            chromium_flags: Vec::new(),
            scheduler: None,
            delegates: Vec::new(),
            renderer_delegates: Vec::new(),
        }
    }

    fn guard_unique_name(&self, name: &str) {
        if self.sync_handlers.contains_key(name)
            || self.async_handlers.contains_key(name)
            || self.stream_handlers.contains_key(name)
        {
            panic!("handler '{name}' registered twice");
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
    /// The closure receives the deserialized payload and a reference to the
    /// shared runtime handle. Use the handle to broadcast events, spawn
    /// background work, or query runtime state.
    ///
    /// Ignore it with _ when not needed.
    ///
    /// Panics if a handler with the same name has already been registered.
    pub fn command<F>(mut self, name: impl Into<String>, f: F) -> Self
    where
        F: Fn(Value, &AppHandle) -> Result<Value, String> + Send + Sync + 'static,
    {
        let name = name.into();
        self.guard_unique_name(&name);
        let cell = self.cell.clone();
        self.sync_handlers.insert(name, Box::new(move |data: &[u8], _ctx: IpcContext| {
            let s = std::str::from_utf8(data).map_err(|e| e.to_string())?;
            let input = if s.is_empty() { Value::Null } else { serde_json::from_str(s).map_err(|e| format!("invalid JSON payload: {e}"))? };
            let output = f(input, cell.get())?;
            serde_json::to_string(&output).map_err(|e| format!("JSON serialization error: {e}")).map(|s| s.into_bytes())
        }));
        self
    }

    /// Registers an asynchronous JSON command handler.
    ///
    /// The closure receives the deserialized payload, an IpcResponder to
    /// send the response later and the shared runtime handle.
    ///
    /// Panics if a handler with the same name has already been registered.
    pub fn async_command<F>(mut self, name: impl Into<String>, f: F) -> Self
    where
        F: Fn(Value, IpcResponder, &AppHandle) + Send + Sync + 'static,
    {
        let name = name.into();
        self.guard_unique_name(&name);
        let cell = self.cell.clone();
        self.async_handlers.insert(name, Box::new(move |data: &[u8], responder: BinaryResponder, _ctx: IpcContext| {
            let s = std::str::from_utf8(data).unwrap_or("");
            let value: Value = if s.is_empty() {
                Value::Null
            } else {
                match serde_json::from_str(s) {
                    Ok(v) => v,
                    Err(e) => {
                        responder.resolve(Err(format!("invalid JSON: {e}")), 0);
                        return;
                    }
                }
            };
            let r = IpcResponder::new(Box::new(move |result, code| {
                responder.resolve(result.map(|s| s.into_bytes()), code);
            }));
            f(value, r, cell.get())
        }));
        self
    }

    /// Registers a synchronous binary command handler.
    ///
    /// The closure receives the raw payload bytes and the shared runtime handle.
    ///
    /// Panics if a handler with the same name has already been registered.
    pub fn binary_command<F>(mut self, name: impl Into<String>, f: F) -> Self
    where
        F: Fn(&[u8], &AppHandle) -> Result<Vec<u8>, String> + Send + Sync + 'static,
    {
        let name = name.into();
        self.guard_unique_name(&name);
        let cell = self.cell.clone();
        self.sync_handlers.insert(name, Box::new(move |data: &[u8], _ctx: IpcContext| {
            f(data, cell.get())
        }));
        self
    }

    /// Registers an asynchronous binary command handler.
    ///
    /// The closure receives the payload bytes (owned), a BinaryResponder to
    /// send the response later and the shared runtime handle.
    ///
    /// Panics if a handler with the same name has already been registered.
    pub fn async_binary_command<F>(mut self, name: impl Into<String>, f: F) -> Self
    where
        F: Fn(Vec<u8>, BinaryResponder, &AppHandle) + Send + Sync + 'static,
    {
        let name = name.into();
        self.guard_unique_name(&name);
        let cell = self.cell.clone();
        self.async_handlers.insert(name, Box::new(move |data: &[u8], responder: BinaryResponder, _ctx: IpcContext| {
            f(data.to_vec(), responder, cell.get())
        }));
        self
    }

    /// Registers a stream handler whose factory does not need AppHandle.
    ///
    /// Stream handlers process data chunks sent from the renderer. The factory
    /// closure is called once per stream open to create a dedicated handler
    /// instance, giving each stream its own mutable state.
    ///
    /// Panics if a handler with the same name is already registered.
    pub fn stream<F, H>(mut self, name: impl Into<String>, factory: F) -> Self
    where
        F: Fn() -> H + Send + Sync + 'static,
        H: crate::ipc::StreamHandler + 'static,
    {
        let name = name.into();
        self.guard_unique_name(&name);
        self.stream_handlers.insert(name, Box::new(move || Box::new(factory())));
        self
    }

    /// Registers a stream handler whose factory receives &AppHandle.
    ///
    /// Identical to stream(Self::stream) but the factory receives a
    /// reference to the shared runtime handle, useful for broadcasting events
    /// or querying runtime state from within stream lifecycle callbacks.
    ///
    /// Panics if a handler with the same name is already registered.
    pub fn stream_h<F, H>(mut self, name: impl Into<String>, factory: F) -> Self
    where
        F: Fn(&AppHandle) -> H + Send + Sync + 'static,
        H: crate::ipc::StreamHandler + 'static,
    {
        let name = name.into();
        self.guard_unique_name(&name);
        let cell = self.cell.clone();
        self.stream_handlers.insert(name, Box::new(move || {
            Box::new(factory(cell.get())) as Box<dyn crate::ipc::StreamHandler>
        }));
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

    /// Initialize CEF and return an AppInstance.
    pub fn build(mut self) -> Result<AppInstance, RuntimeError> {
        let resolver = self.resolver.take().expect("build called twice");

        let Self {
            source,
            sync_handlers,
            async_handlers,
            stream_handlers,
            profile_id,
            persist_session_cookies,
            gpu_mode,
            chromium_flags,
            scheduler,
            delegates,
            renderer_delegates,
            ..
        } = self;

        let rpc = RequestResponseSubsystem::new(sync_handlers, async_handlers);
        let event = EventSubsystem::new();
        let stream = StreamSubsystem::new(stream_handlers);
        let router = Arc::new(IpcRouter::new(rpc, event, stream));

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

        let instance = RuntimeBootstrap::start(spec, router)?;
        // Populated before the message loop starts
        resolver.resolve(instance.handle().clone());
        Ok(instance)
    }

    /// Starts the runtime in embedded mode.
    pub fn start_embedded(mut self) -> Result<AppInstance, RuntimeError> {
        let resolver = self.resolver.take().expect("start_embedded called twice");

        let Self {
            source,
            sync_handlers,
            async_handlers,
            stream_handlers,
            profile_id,
            persist_session_cookies,
            gpu_mode,
            chromium_flags,
            scheduler,
            delegates,
            renderer_delegates,
            ..
        } = self;

        let rpc = RequestResponseSubsystem::new(sync_handlers, async_handlers);
        let event = EventSubsystem::new();
        let stream = StreamSubsystem::new(stream_handlers);
        let router = Arc::new(IpcRouter::new(rpc, event, stream));

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

    fn json_noop(_: Value, _: &AppHandle) -> Result<Value, String> {
        Ok(Value::Null)
    }

    fn binary_noop(_: &[u8], _: &AppHandle) -> Result<Vec<u8>, String> {
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

    #[test]
    #[should_panic(expected = "registered twice")]
    fn duplicate_async_command_panics() {
        App::new("./dist")
            .command("go", json_noop)
            .async_command("go", |_: Value, r: IpcResponder, _: &AppHandle| {
                r.resolve(Ok("ok".into()), 0);
            });
    }

    #[test]
    #[should_panic(expected = "registered twice")]
    fn async_and_json_same_name_panics() {
        App::new("./dist")
            .async_command("task", |_: Value, r: IpcResponder, _: &AppHandle| {
                r.resolve(Ok("ok".into()), 0);
            })
            .command("task", json_noop);
    }

    #[test]
    #[should_panic(expected = "registered twice")]
    fn async_and_binary_same_name_panics() {
        App::new("./dist")
            .async_command("upload", |_: Value, r: IpcResponder, _: &AppHandle| {
                r.resolve(Ok("ok".into()), 0);
            })
            .binary_command("upload", binary_noop);
    }

    #[test]
    #[should_panic(expected = "registered twice")]
    fn stream_duplicate_panics() {
        struct NoopStream;
        impl crate::ipc::StreamHandler for NoopStream {
            fn on_open(&mut self, _: &str, _: &crate::ipc::StreamResponder) -> Result<(), String> { Ok(()) }
            fn on_chunk(&mut self, _: &[u8], _: &crate::ipc::StreamResponder) -> Result<(), String> { Ok(()) }
            fn on_end(&mut self, _: &str, _: crate::ipc::StreamResponder) -> Result<(), String> { Ok(()) }
            fn on_error(&mut self, _: &str) {}
        }
        App::new("./dist")
            .stream("data", || NoopStream)
            .stream("data", || NoopStream);
    }

    #[test]
    #[should_panic(expected = "registered twice")]
    fn stream_and_command_same_name_panics() {
        struct NoopStream;
        impl crate::ipc::StreamHandler for NoopStream {
            fn on_open(&mut self, _: &str, _: &crate::ipc::StreamResponder) -> Result<(), String> { Ok(()) }
            fn on_chunk(&mut self, _: &[u8], _: &crate::ipc::StreamResponder) -> Result<(), String> { Ok(()) }
            fn on_end(&mut self, _: &str, _: crate::ipc::StreamResponder) -> Result<(), String> { Ok(()) }
            fn on_error(&mut self, _: &str) {}
        }
        App::new("./dist")
            .stream("data", || NoopStream)
            .command("data", json_noop);
    }

    #[test]
    #[should_panic(expected = "registered twice")]
    fn command_and_stream_same_name_panics() {
        struct NoopStream;
        impl crate::ipc::StreamHandler for NoopStream {
            fn on_open(&mut self, _: &str, _: &crate::ipc::StreamResponder) -> Result<(), String> { Ok(()) }
            fn on_chunk(&mut self, _: &[u8], _: &crate::ipc::StreamResponder) -> Result<(), String> { Ok(()) }
            fn on_end(&mut self, _: &str, _: crate::ipc::StreamResponder) -> Result<(), String> { Ok(()) }
            fn on_error(&mut self, _: &str) {}
        }
        App::new("./dist")
            .command("data", json_noop)
            .stream("data", || NoopStream);
    }

    #[test]
    #[should_panic(expected = "registered twice")]
    fn stream_and_async_same_name_panics() {
        struct NoopStream;
        impl crate::ipc::StreamHandler for NoopStream {
            fn on_open(&mut self, _: &str, _: &crate::ipc::StreamResponder) -> Result<(), String> { Ok(()) }
            fn on_chunk(&mut self, _: &[u8], _: &crate::ipc::StreamResponder) -> Result<(), String> { Ok(()) }
            fn on_end(&mut self, _: &str, _: crate::ipc::StreamResponder) -> Result<(), String> { Ok(()) }
            fn on_error(&mut self, _: &str) {}
        }
        App::new("./dist")
            .stream("task", || NoopStream)
            .async_command("task", |_: Value, r: IpcResponder, _: &AppHandle| {
                r.resolve(Ok("ok".into()), 0);
            });
    }
}
