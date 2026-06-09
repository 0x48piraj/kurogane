#![deny(unused_must_use)]
#![deny(unused_variables)]
#![deny(dead_code)]

mod runtime;
mod app;
mod cef_app;
mod browser;
mod window;
mod client;
mod scheme;
mod error;
mod fs;
mod chromium_flags;
mod sandbox;
mod gpu;
pub mod message_loop;
pub mod ipc;
pub mod bridge;
pub mod logger;

#[cfg(target_os = "macos")]
mod platform;

pub use runtime::{Runtime, RuntimeHandle};
pub use gpu::GpuMode;
pub use error::RuntimeError;
pub use app::App;
pub use message_loop::MessageLoopMode;

// Re-export IPC types for public use
pub use crate::ipc::IpcResult;
