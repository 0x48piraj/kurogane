//! A composable Chromium runtime for Rust.
//!
//! Build high-performance, GPU-accelerated desktop applications on Chromium or
//! embed it directly into existing applications.
//!
//! Guides, concepts and the CLI reference are at <https://kurogane-rs.org/docs>.
//!
//! # A first app
//!
//! ```no_run
//! use kurogane::{App, AppHandle, IpcError};
//!
//! fn greet(name: String, _: &AppHandle) -> Result<String, IpcError> {
//!     Ok(format!("Hello, {name}!"))
//! }
//!
//! fn main() {
//!     App::new("content").command("greet", greet).run_or_exit();
//! }
//! ```
//!
//! The page calls the command with `window.kurogane.invoke("greet", "Kurogane")`
//! and gets `"Hello, Kurogane!"` back.
//!
//! # Where to start
//!
//! * [`App`] describes an application and starts it.
//! * [`AppInstance`] is the running application, owned by the thread that
//!   started it. [`AppHandle`] reaches it from any thread.
//! * [`BrowserHandle`] drives one browser.
//! * [`WindowOptions`] describes an application window.
//!
//! # Talking to the page
//!
//! * [`App::command`] registers a command. Its async and binary forms answer
//!   later or take bytes. A handler fails with an [`IpcError`].
//! * [`AppHandle::broadcast_json`] sends an event to every subscribed page.
//! * [`App::stream`] registers a [`StreamHandler`] for chunks in both
//!   directions.
//! * [`App::permit`] opens a command or stream to another origin.
//! * [`capability::Filesystem`] grants pages scoped access to files.
//!
//! # Deciding what a page may do
//!
//! Hooks such as [`App::on_new_window`], [`App::on_navigation`],
//! [`App::on_download`] and [`App::on_permission`] take a typed request. Each
//! answers with a decision whose `Default` leaves the choice to Kurogane.
//!
//! # Processes
//!
//! Chromium runs your binary again for each of its helper processes. Code
//! before the call that starts the runtime runs in every process. Guard
//! one-time work with [`is_browser_process`]. See [`App`].
//!
//! # Your own event loop
//!
//! [`App::start`] and [`App::start_embedded`] start Chromium without its
//! message loop. Your loop calls [`AppInstance::pump`] until
//! [`AppInstance::should_shutdown`] turns true and then
//! [`AppInstance::shutdown`].

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
mod window_closing;
mod cef_string;
mod client;
mod hooks;
mod destination;
mod new_window;
mod navigation;
mod keys;
mod shortcuts;
mod downloads;
mod permissions;
mod file_dialog;
mod drag;
mod page_events;
mod external;
mod chrome_commands;
mod context_menu;
mod scheme;
mod error;
mod fs;
mod resources;
mod chromium_defaults;
mod chromium_flags;
mod sandbox;
mod gpu;
mod credentials;
mod ipc;
mod bridge;
pub mod capability;

mod platform;

pub use runtime::{AppInstance, AppHandle, BrowserBounds, BrowserHandle};
pub use window_options::{WindowOptions, WindowPlacement, WindowState};
pub use window_closing::WindowClosing;
pub use runtime::is_browser_process;
pub use browser_registry::{BrowserId, BrowserMetadata, BrowserType};
pub use window_registry::{WindowId, WindowMetadata};
pub use gpu::GpuMode;
pub use credentials::CredentialStorage;
pub use spec::SandboxMode;
pub use scheme::{SchemeHandler, resource_handler_from_bytes};
pub use error::{CefLocation, ConfigError, RuntimeError};
pub use acl::{Origin, OriginError};
pub use app::App;
pub use resources::resource_dir;
/// Window-handle traits [`AppInstance::create_child_browser`] takes, at the
/// version Kurogane uses. A host passes its window as it is.
pub use raw_window_handle;
/// tanso, Kurogane's CEF bindings, at the revision Kurogane is built with.
pub use tanso;

/// What Kurogane's macros expand to. Not a public API.
#[doc(hidden)]
pub mod __private {
    #[cfg(target_os = "windows")]
    pub use crate::sandbox::windows::enter;
}

// What handlers take and return
pub use crate::ipc::{BinaryResponder, ErrorCode, IpcError, Responder, StreamHandler, StreamResponder};
pub use app::{PumpRequest, ClientAppBrowserDelegate, ClientAppRendererDelegate, SecondInstance};
pub use new_window::{NewWindowDecision, NewWindowKind, NewWindowRequest};
pub use navigation::{NavigationDecision, NavigationRequest};
pub use keys::{Key, KeyDecision, KeyPress, Modifiers};
pub use chrome_commands::{ChromeCommand, ChromeCommandRequest, CommandDecision};
pub use downloads::{DownloadDecision, DownloadRequest};
pub use permissions::{Permission, PermissionDecision, PermissionRequest, PermissionResponder};
pub use file_dialog::{FileDialogDecision, FileDialogKind, FileDialogRequest, FileDialogResponder};
pub use drag::{DragDecision, DragEnter};
pub use page_events::{FullscreenChange, TitleChange};
pub use context_menu::{
    AppItem, ContextMenu, ContextMenuCommand, ContextMenuTarget, MediaKind, MenuItem, StandardItem,
    Submenu,
};
