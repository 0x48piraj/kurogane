//! The application's hooks: closures Kurogane calls from CEF's callbacks to
//! let the application decide.
//!
//! The startup spec holds them; the running application's services only
//! point to them. A hook may keep an [`AppHandle`], and the services every
//! handle shares must not keep the hook in turn, or neither would ever be
//! released. CEF releases the spec when it shuts down, which ends the hooks
//! and any handle they hold; a hook asked for after that is not there.

use crate::chrome_commands::{ChromeCommandRequest, CommandDecision};
use crate::keys::{KeyDecision, KeyPress};
use crate::navigation::{NavigationDecision, NavigationRequest};
use crate::new_window::{NewWindowDecision, NewWindowRequest};
use crate::runtime::AppHandle;

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

/// The hooks the application registered.
#[derive(Default)]
pub(crate) struct Hooks {
    pub new_window: Option<NewWindowHook>,
    pub navigation: Option<NavigationHook>,
    pub key: Option<KeyHook>,
    pub chrome_command: Option<ChromeCommandHook>,
}
