//! High level application bootstrap API.
//!
//! This is the public developer entrypoint built on top of Runtime.
//! This helps in the abstraction of asset resolution, environment overrides and command registration.

use std::path::PathBuf;
use std::sync::Arc;
use serde_json::Value;
use std::collections::HashMap;
use crate::app::resolver::ResolvedFrontend;
use crate::ipc::browser_state::{IpcDispatcher, IpcHandler, BinaryHandler};
use crate::{Runtime, RuntimeError};
use crate::chromium_flags::ChromiumFlag;
use crate::gpu::GpuMode;

mod resolver;

/// Describes where the frontend comes from
pub(crate) enum Source {
    Url(String),
    Path(PathBuf),
}

/// Public application builder.
///
/// Configures how the first browser instance starts.
pub struct App {
    source: Source,
    commands: HashMap<String, IpcHandler>,
    binary_commands: HashMap<String, BinaryHandler>,

    profile_id: Option<String>,
    persist_session_cookies: bool,
    gpu_mode: GpuMode,
    chromium_flags: Vec<ChromiumFlag>,
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
            commands: HashMap::new(),
            binary_commands: HashMap::new(),

            profile_id: None,
            persist_session_cookies: true,
            gpu_mode: GpuMode::Auto,
            chromium_flags: Vec::new(),
        }
    }

    /// Register a JSON command handler.
    ///
    /// Panics if name has already been registered.
    pub fn command<F>(mut self, name: impl Into<String>, handler: F) -> Self
    where
        F: Fn(Value) -> Result<Value, String> + Send + Sync + 'static,
    {
        let name = name.into();

        assert!(
            !self.commands.contains_key(&name) && !self.binary_commands.contains_key(&name),
            "command '{name}' registered twice"
        );

        // Wrap the typed handler into the wire-level IpcHandler once.
        let wrapped: IpcHandler = Box::new(move |payload: &str| {
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

        self.commands.insert(name, wrapped);
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

        assert!(
            !self.commands.contains_key(&name) && !self.binary_commands.contains_key(&name),
            "command '{name}' registered twice"
        );

        self.binary_commands.insert(name, Box::new(handler));
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

    /// Override GPU backend selection
    pub fn gpu_mode(mut self, mode: GpuMode) -> Self {
        self.gpu_mode = mode;
        self
    }

    /// Add a Chromium flag with no value
    pub fn chromium_flag(mut self, name: impl Into<String>) -> Self {
        self.chromium_flags.push(ChromiumFlag::Present(name.into()));
        self
    }

    /// Add a Chromium flag with a value
    pub fn chromium_flag_with_value(
        mut self,
        name: impl Into<String>,
        value: impl Into<String>,
    ) -> Self {
        self.chromium_flags
            .push(ChromiumFlag::WithValue(name.into(), value.into()));
        self
    }

    /// Start the application
    pub fn run(self) -> Result<(), RuntimeError> {
        let ResolvedFrontend { asset_root, start_url } = resolver::resolve(&self.source)?;

        // Freeze IPC configuration into an immutable dispatcher shared across the runtime object graph.
        let dispatcher = Arc::new(IpcDispatcher::new(self.commands, self.binary_commands));

        Runtime::run(
            start_url,
            asset_root,
            dispatcher,
            self.profile_id,
            self.persist_session_cookies,
            self.gpu_mode,
            self.chromium_flags,
        )
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
