//! High level application bootstrap API.
//!
//! This is the public developer entrypoint built on top of Runtime.
//! This helps in the abstraction of asset resolution, environment overrides and command registration.

use std::path::PathBuf;
use serde_json::Value;
use crate::app::resolver::ResolvedFrontend;

use crate::{Runtime, RuntimeError, register_command, register_binary_command};

mod resolver;

type CommandHandler =
    Box<dyn Fn(Value) -> Result<Value, String> + Send + Sync + 'static>;

type BinaryHandler =
    Box<dyn Fn(&[u8]) -> Result<Vec<u8>, String> + Send + Sync + 'static>;

/// Describes where the frontend comes from
enum Source {
    Url(String),
    Path(PathBuf),
}

/// Public application builder.
///
/// This only configures how the first browser instance starts.
pub struct App {
    source: Source,
    commands: Vec<(String, CommandHandler)>,
    binary_commands: Vec<(String, BinaryHandler)>,

    profile_id: Option<String>,
    persist_session_cookies: bool,
}

impl App {
    /// Create an app from a local directory (default entrypoint)
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            source: Source::Path(path.into()),
            commands: Vec::new(),
            binary_commands: Vec::new(),

            profile_id: None,
            persist_session_cookies: true,
        }
    }

    /// Start from an explicit URL (escape hatch for power users)
    pub fn url(url: impl Into<String>) -> Self {
        Self {
            source: Source::Url(url.into()),
            commands: Vec::new(),
            binary_commands: Vec::new(),

            profile_id: None,
            persist_session_cookies: true,
        }
    }

    /// Register an IPC command
    pub fn command<F>(mut self, name: impl Into<String>, handler: F) -> Self
    where
        F: Fn(Value) -> Result<Value, String> + Send + Sync + 'static,
    {
        self.commands.push((name.into(), Box::new(handler)));
        self
    }

    pub fn binary_command<F>(
        mut self,
        name: impl Into<String>,
        handler: F,
    ) -> Self
    where
        F: Fn(&[u8]) -> Result<Vec<u8>, String> + Send + Sync + 'static,
    {
        self.binary_commands
            .push((name.into(), Box::new(handler)));
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

    /// Start the application
    pub fn run(self) -> Result<(), RuntimeError> {
        let ResolvedFrontend { asset_root, start_url } = resolver::resolve(&self.source)?;

        for (name, handler) in self.commands {
            register_command(name, handler);
        }

        for (name, handler) in self.binary_commands {
            register_binary_command(name, handler);
        }

        Runtime::run(
            start_url,
            asset_root,
            self.profile_id,
            self.persist_session_cookies,
        )
    }

    /// Run the application and terminate the process on failure.
    /// Intended for binaries. Libraries embedding the runtime should use run() instead.
    pub fn run_or_exit(self) {
        if let Err(e) = self.run() {
            eprintln!("\nApplication failed to start:\n{}\n", e);
            std::process::exit(1);
        }
    }
}
