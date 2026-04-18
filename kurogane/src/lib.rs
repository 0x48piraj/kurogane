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
pub mod ipc;
pub mod logger;

#[cfg(target_os = "macos")]
mod platform;

pub use runtime::Runtime;
pub use error::RuntimeError;
pub use app::App;

// Re-export IPC types for public use
pub use crate::ipc::{
    IpcResult,
    register_command,
    register_binary_command
};
