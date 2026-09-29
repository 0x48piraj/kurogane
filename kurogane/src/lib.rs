#![deny(unused_must_use)]
#![deny(unused_variables)]
#![deny(dead_code)]

mod runtime;
mod spec;
mod acl;
mod app;
mod cef_app;
mod browser;
mod browser_registry;
mod window_registry;
mod registry;
mod window;
mod client;
mod chrome_commands;
mod scheme;
mod error;
mod fs;
mod resources;
mod chromium_flags;
mod sandbox;
mod gpu;
mod credentials;
mod shutdown;
pub mod ipc;
pub mod bridge;
pub mod logger;
pub mod capability;

mod platform;

pub use runtime::{AppInstance, AppHandle, BrowserBounds, BrowserHandle, WindowOptions, WindowState};
pub use runtime::is_browser_process;
pub use browser_registry::{BrowserId, BrowserMetadata, BrowserType};
pub use window_registry::{WindowId, WindowMetadata};
pub use gpu::GpuMode;
pub use credentials::CredentialStorage;
pub use spec::SandboxMode;
pub use scheme::{
    CustomScheme, ResolveError, ResolvedAsset, SchemeHandler, resource_handler_from_bytes,
};
pub use error::{ConfigError, RuntimeError};
pub use acl::{Origin, OriginError};
pub use app::App;
pub use resources::resource_dir;
pub use shutdown::ShutdownSignal;

/// What Kurogane's macros expand to. Not a public API.
#[doc(hidden)]
pub mod __private {
    #[cfg(target_os = "windows")]
    pub use crate::sandbox::windows::enter;
}

// Re-export IPC types for public use
pub use crate::ipc::{ErrorCode, IpcError, Responder};
pub use app::{
    PumpRequest, PumpScheduler, ClientAppBrowserDelegate, ClientAppRendererDelegate, SecondInstance,
};
