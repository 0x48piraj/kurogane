#![deny(unused_must_use)]
#![deny(unused_variables)]
#![deny(dead_code)]

mod runtime;
mod spec;
mod app;
mod cef_app;
mod browser;
mod browser_registry;
mod window_registry;
mod window;
mod client;
mod scheme;
mod error;
mod fs;
mod chromium_flags;
mod sandbox;
mod gpu;
mod shutdown;
pub mod ipc;
pub mod bridge;
pub mod logger;
mod browser_info_map;

#[cfg(target_os = "macos")]
mod platform;

pub use runtime::{RuntimeHandle, BrowserBounds, BrowserHandle, WindowOptions, WindowState};
pub use browser_registry::{BrowserId, BrowserMetadata, BrowserType};
pub use window_registry::{WindowId, WindowMetadata};
pub use gpu::GpuMode;
pub use error::RuntimeError;
pub use app::App;
pub use shutdown::ShutdownSignal;

// Re-export IPC types for public use
pub use crate::ipc::IpcResult;
pub use app::{PumpRequest, PumpScheduler, ClientAppBrowserDelegate, ClientAppRendererDelegate};
pub use browser_info_map::{BrowserInfoMap, BrowserInfoMapVisitor, BrowserInfoMapVisitorResult};
