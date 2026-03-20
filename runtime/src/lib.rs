#![deny(unused_must_use)]
#![deny(unused_variables)]
#![deny(dead_code)]

//! Kurogane
//!
//! Minimal Rust runtime for building Chromium-based desktop apps using CEF.
//! Provides a bootstrap API while exposing CEF underneath.

mod runtime;
mod app;
mod cef_app;
mod browser;
mod window;
mod client;
mod scheme;
mod error;
mod ipc_renderer;
mod ipc_browser;
mod ipc_shm;
pub mod logger;

#[cfg(target_os = "macos")]
mod platform;

pub use runtime::Runtime;
pub use error::RuntimeError;
pub use app::App;

// Re-export IPC types for public use
pub use ipc_browser::{IpcResult, register_command, register_binary_command};
