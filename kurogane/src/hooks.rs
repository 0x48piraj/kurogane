//! The application's hooks: closures Kurogane calls from CEF's callbacks to
//! let the application decide.
//!
//! The startup spec holds them; the running application's services only
//! point to them. A hook may keep an [`AppHandle`], and the services every
//! handle shares must not keep the hook in turn, or neither would ever be
//! released. CEF releases the spec when it shuts down, which ends the hooks
//! and any handle they hold; a hook asked for after that is not there.

use crate::chrome_commands::{ChromeCommandRequest, CommandDecision};
use crate::context_menu::{ContextMenu, ContextMenuCommand};
use crate::downloads::{DownloadDecision, DownloadRequest};
use crate::drag::{DragDecision, DragEnter};
use crate::file_dialog::{FileDialogDecision, FileDialogRequest};
use crate::keys::{KeyDecision, KeyPress};
use crate::navigation::{NavigationDecision, NavigationRequest};
use crate::new_window::{NewWindowDecision, NewWindowRequest};
use crate::page_events::{FullscreenChange, TitleChange};
use crate::permissions::{PermissionDecision, PermissionRequest};
use crate::runtime::AppHandle;
use crate::window_closing::WindowClosing;

/// What [`App::on_new_window`](crate::App::on_new_window) stores.
pub(crate) type NewWindowHook =
    Box<dyn Fn(&NewWindowRequest, &AppHandle) -> NewWindowDecision + Send + Sync>;

/// What [`App::on_navigation`](crate::App::on_navigation) stores.
pub(crate) type NavigationHook =
    Box<dyn Fn(&NavigationRequest, &AppHandle) -> NavigationDecision + Send + Sync>;

/// What [`App::on_key`](crate::App::on_key) stores.
pub(crate) type KeyHook = Box<dyn Fn(&KeyPress, &AppHandle) -> KeyDecision + Send + Sync>;

/// What [`App::on_chrome_command`](crate::App::on_chrome_command) stores.
pub(crate) type ChromeCommandHook =
    Box<dyn Fn(&ChromeCommandRequest, &AppHandle) -> CommandDecision + Send + Sync>;

/// What [`App::on_download`](crate::App::on_download) stores.
pub(crate) type DownloadHook =
    Box<dyn Fn(&DownloadRequest, &AppHandle) -> DownloadDecision + Send + Sync>;

/// What [`App::on_permission`](crate::App::on_permission) stores.
pub(crate) type PermissionHook =
    Box<dyn Fn(&PermissionRequest, &AppHandle) -> PermissionDecision + Send + Sync>;

/// What [`App::on_context_menu`](crate::App::on_context_menu) stores.
pub(crate) type ContextMenuHook = Box<dyn Fn(&mut ContextMenu, &AppHandle) + Send + Sync>;

/// What [`App::on_context_menu_command`](crate::App::on_context_menu_command)
/// stores.
pub(crate) type ContextMenuCommandHook = Box<dyn Fn(&ContextMenuCommand, &AppHandle) + Send + Sync>;

/// What [`App::on_window_closing`](crate::App::on_window_closing) stores.
pub(crate) type WindowClosingHook = Box<dyn Fn(&WindowClosing, &AppHandle) + Send + Sync>;

/// What [`App::on_file_dialog`](crate::App::on_file_dialog) stores.
pub(crate) type FileDialogHook =
    Box<dyn Fn(&FileDialogRequest, &AppHandle) -> FileDialogDecision + Send + Sync>;

/// What [`App::on_drag_enter`](crate::App::on_drag_enter) stores.
pub(crate) type DragEnterHook = Box<dyn Fn(&DragEnter, &AppHandle) -> DragDecision + Send + Sync>;

/// What [`App::on_title_change`](crate::App::on_title_change) stores.
pub(crate) type TitleChangeHook = Box<dyn Fn(&TitleChange, &AppHandle) + Send + Sync>;

/// What [`App::on_fullscreen_change`](crate::App::on_fullscreen_change)
/// stores.
pub(crate) type FullscreenChangeHook = Box<dyn Fn(&FullscreenChange, &AppHandle) + Send + Sync>;

/// The hooks the application registered.
#[derive(Default)]
pub(crate) struct Hooks {
    pub new_window: Option<NewWindowHook>,
    pub navigation: Option<NavigationHook>,
    pub key: Option<KeyHook>,
    pub chrome_command: Option<ChromeCommandHook>,
    pub download: Option<DownloadHook>,
    pub permission: Option<PermissionHook>,
    pub context_menu: Option<ContextMenuHook>,
    pub context_menu_command: Option<ContextMenuCommandHook>,
    pub window_closing: Option<WindowClosingHook>,
    pub file_dialog: Option<FileDialogHook>,
    pub drag_enter: Option<DragEnterHook>,
    pub title_change: Option<TitleChangeHook>,
    pub fullscreen_change: Option<FullscreenChangeHook>,
}
