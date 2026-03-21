//! High level application bootstrap API.
//!
//! This is the public developer entrypoint built on top of Runtime.
//! This helps in the abstraction of asset resolution, environment overrides and command registration.

use std::path::PathBuf;
use serde_json::Value;

use crate::{Runtime, RuntimeError, register_command, register_binary_command};

mod resolver;

type CommandHandler =
    Box<dyn Fn(Value) -> Result<Value, String> + Send + Sync + 'static>;

type BinaryHandler =
    Box<dyn Fn(&[u8]) -> Result<Vec<u8>, String> + Send + Sync + 'static>;

/// Describes where the frontend comes from
enum Source {
    Name(String),
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
}

impl App {
    /// Create an app from a name OR URL
    pub fn new(input: impl Into<String>) -> Self {
        let input = input.into();

        let source = if input.starts_with("http://") || input.starts_with("https://") {
            Source::Url(input)
        } else {
            Source::Name(input)
        };

        Self {
            source,
            commands: Vec::new(),
            binary_commands: Vec::new(),
        }
    }

    /// Start from an explicit URL (escape hatch for power users)
    pub fn url(url: impl Into<String>) -> Self {
        Self {
            source: Source::Url(url.into()),
            commands: Vec::new(),
            binary_commands: Vec::new(),
        }
    }

    /// Load frontend from explicit directory (escape hatch for power users)
    pub fn path(path: impl Into<PathBuf>) -> Self {
        Self {
            source: Source::Path(path.into()),
            commands: Vec::new(),
            binary_commands: Vec::new(),
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

    /// Start the application
    pub fn run(self) -> Result<(), RuntimeError> {
        let (asset_root, url) = resolver::resolve(&self.source);

        let require_assets = asset_root.is_some();

        if let Some(root) = asset_root {
            Runtime::set_asset_root(root)?;
        }

        for (name, handler) in self.commands {
            register_command(name, handler);
        }

        for (name, handler) in self.binary_commands {
            register_binary_command(name, handler);
        }

        Runtime::run(url, require_assets)
    }

    /// Run the application and terminate the process on failure.
    ///
    /// Intended for binaries and examples.
    /// Libraries embedding the runtime should use run() instead.
    pub fn run_or_exit(self) {
        if let Err(e) = self.run() {
            eprintln!("\nApplication failed to start:\n{}\n", e);
            std::process::exit(1);
        }
    }
}
