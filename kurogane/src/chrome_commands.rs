//! CEF command filtering for Kurogane.
//!
//! A Kurogane window omits standard browser UI (tab strips, toolbars, app menus).
//! However, the underlying CEF framework still processes default Chrome shortcuts
//! (e.g., Ctrl+N, Ctrl+T) and context menus.
//!
//! To prevent CEF from spawning unmanaged native windows or tabs, this module intercepts
//! the command pipeline. Kurogane uses a strict allowlist: commands execute ONLY if they
//! operate on the current page context. All other commands are intentionally swallowed.
//! On macOS, Chrome's quit command maps ⌘Q to application termination.

use std::ffi::{CStr, c_char, c_int};
use std::sync::OnceLock;

use cef::*;

use tracing::debug;

/// Strict allowlist of page-local commands, by their names in `cef_command_ids.h`.
const ALLOWED: &[&CStr] = &[
    // Navigation within the page
    c"IDC_BACK",
    c"IDC_FORWARD",
    c"IDC_RELOAD",
    c"IDC_RELOAD_BYPASSING_CACHE",
    c"IDC_RELOAD_CLEARING_CACHE",
    c"IDC_STOP",
    // Zoom, fullscreen, find and print
    c"IDC_ZOOM_PLUS",
    c"IDC_ZOOM_NORMAL",
    c"IDC_ZOOM_MINUS",
    c"IDC_FULLSCREEN",
    c"IDC_FIND",
    c"IDC_FIND_NEXT",
    c"IDC_FIND_PREVIOUS",
    c"IDC_PRINT",
    // Editing, from shortcuts and from the context menu
    c"IDC_CUT",
    c"IDC_COPY",
    c"IDC_PASTE",
    c"IDC_CONTENT_CONTEXT_CUT",
    c"IDC_CONTENT_CONTEXT_COPY",
    c"IDC_CONTENT_CONTEXT_PASTE",
    c"IDC_CONTENT_CONTEXT_PASTE_AND_MATCH_STYLE",
    c"IDC_CONTENT_CONTEXT_DELETE",
    c"IDC_CONTENT_CONTEXT_SELECTALL",
    c"IDC_CONTENT_CONTEXT_UNDO",
    c"IDC_CONTENT_CONTEXT_REDO",
    // Closing the window the command came from
    c"IDC_CLOSE_TAB",
    c"IDC_CLOSE_WINDOW",
    // Developer tools, which open as Kurogane popups
    c"IDC_DEV_TOOLS",
    c"IDC_DEV_TOOLS_CONSOLE",
    c"IDC_DEV_TOOLS_INSPECT",
    c"IDC_DEV_TOOLS_TOGGLE",
    c"IDC_CONTENT_CONTEXT_INSPECTELEMENT",
];

unsafe extern "C" {
    /// Resolves a string command name to its dynamic CEF command ID.
    /// Returns `-1` if the command is absent in the current CEF build.
    fn cef_id_for_command_id_name(name: *const c_char) -> c_int;
}

/// Resolves a stable command name to its dynamic runtime ID.
fn command_id(name: &CStr) -> Option<c_int> {
    // SAFETY: `name` is a valid, null-terminated C-string. The FFI boundary
    // guarantees read-only access and the backing memory outlives the call.
    let id = unsafe { cef_id_for_command_id_name(name.as_ptr()) };
    (id >= 0).then_some(id)
}

/// Translates [`ALLOWED`] command names to runtime IDs.
/// Cached via [`OnceLock`] because CEF command IDs are unstable across Chromium
/// versions, whereas string names provide a stable resolution ABI.
fn allowed_ids() -> &'static [c_int] {
    static IDS: OnceLock<Vec<c_int>> = OnceLock::new();
    IDS.get_or_init(|| ALLOWED.iter().filter_map(|name| command_id(name)).collect())
}

/// Resolves the dynamic ID range reserved for application-defined context menu items.
fn custom_context_ids() -> Option<(c_int, c_int)> {
    static RANGE: OnceLock<Option<(c_int, c_int)>> = OnceLock::new();
    *RANGE.get_or_init(|| {
        Some((
            command_id(c"IDC_CONTENT_CONTEXT_CUSTOM_FIRST")?,
            command_id(c"IDC_CONTENT_CONTEXT_CUSTOM_LAST")?,
        ))
    })
}

/// Returns Chrome's quit command ID on macOS.
#[cfg(target_os = "macos")]
fn exit_id() -> Option<c_int> {
    static ID: OnceLock<Option<c_int>> = OnceLock::new();
    *ID.get_or_init(|| command_id(c"IDC_EXIT"))
}

/// Returns whether a Kurogane window runs the command `id`.
fn allowed(id: c_int, disposition: WindowOpenDisposition) -> bool {
    disposition == WindowOpenDisposition::CURRENT_TAB
        && (allowed_ids().contains(&id)
            || custom_context_ids().is_some_and(|(first, last)| (first..=last).contains(&id)))
}

// Leave CEF's four `is_chrome_*` callbacks at their defaults. cef-rs returns
// 0 for these callbacks, whereas CEF's C++ defaults return true. For a
// Chrome-style window, this keeps the app menu items, page actions and
// toolbar buttons hidden when Chromium creates them itself.
wrap_command_handler! {
    pub struct KuroganeCommandHandler;

    impl CommandHandler {
        fn on_chrome_command(
            &self,
            _browser: Option<&mut Browser>,
            command_id: c_int,
            disposition: WindowOpenDisposition,
        ) -> c_int {
            // Allow macOS's quit command to terminate the application
            #[cfg(target_os = "macos")]
            if Some(command_id) == exit_id() {
                debug!("[Commands] Chrome's quit command: quitting as the menu's Quit does");
                crate::platform::macos::quit();
                return 1;
            }

            if allowed(command_id, disposition) {
                return 0; // False. Unhandled by wrapper, proceed with default CEF execution.
            }

            debug!("[Commands] refused Chrome command {command_id} ({disposition:?})");

            1 // True. Handled by wrapper. Swallows the command so CEF drops it.
        }
    }
}
