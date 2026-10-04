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
mod window_options;
mod client;
mod hooks;
mod destination;
mod new_window;
mod navigation;
mod keys;
mod downloads;
mod permissions;
mod external;
mod chrome_commands;
mod context_menu;
mod scheme;
mod error;
mod fs;
mod resources;
mod chromium_flags;
mod sandbox;
mod gpu;
mod credentials;
mod ipc;
mod bridge;
pub mod capability;

mod platform;

pub use runtime::{AppInstance, AppHandle, BrowserBounds, BrowserHandle};
pub use window_options::{WindowOptions, WindowState};
pub use runtime::is_browser_process;
pub use browser_registry::{BrowserId, BrowserMetadata, BrowserType};
pub use window_registry::{WindowId, WindowMetadata};
pub use gpu::GpuMode;
pub use credentials::CredentialStorage;
pub use spec::SandboxMode;
pub use scheme::{SchemeHandler, resource_handler_from_bytes};
pub use error::{ConfigError, RuntimeError};
pub use acl::{Origin, OriginError};
pub use app::App;
pub use resources::resource_dir;
/// The window-handle traits [`AppInstance::create_child_browser`] takes, at
/// the version Kurogane uses: a host passes its window as it is.
pub use raw_window_handle;
/// cef-rs, at the revision Kurogane is built with.
pub use cef;

/// What Kurogane's macros expand to. Not a public API.
#[doc(hidden)]
pub mod __private {
    #[cfg(target_os = "windows")]
    pub use crate::sandbox::windows::enter;
}

// What handlers take and return
pub use crate::ipc::{BinaryResponder, ErrorCode, IpcError, Responder, StreamHandler, StreamResponder};
pub use app::{PumpRequest, ClientAppBrowserDelegate, ClientAppRendererDelegate, SecondInstance};
pub use new_window::{NewWindowDecision, NewWindowRequest};
pub use navigation::{NavigationDecision, NavigationRequest};
pub use keys::{Key, KeyDecision, KeyPress, Modifiers};
pub use chrome_commands::{ChromeCommand, ChromeCommandRequest, CommandDecision};
pub use downloads::{DownloadDecision, DownloadRequest};
pub use permissions::{Permission, PermissionDecision, PermissionRequest, PermissionResponder};
pub use context_menu::{
    AppItem, ContextMenu, ContextMenuCommand, ContextMenuTarget, MediaKind, MenuItem, StandardItem,
    Submenu,
};
