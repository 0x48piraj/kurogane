//! CEF command filtering for Kurogane.
//!
//! A Kurogane window omits standard browser UI (tab strips, toolbars, app menus).
//! However, the underlying CEF framework still processes default Chrome shortcuts
//! (e.g., Ctrl+N, Ctrl+T). The context menu is Kurogane's own
//! (`crate::context_menu`), whose standard items ask the application as
//! these commands do.
//!
//! To prevent CEF from spawning unmanaged native windows or tabs, this module intercepts
//! the command pipeline. Kurogane uses a strict allowlist: commands execute ONLY if they
//! operate on the current page context. All other commands are intentionally swallowed.
//! On macOS, Chrome's quit command maps ⌘Q to application termination.
//!
//! The application's [`App::on_chrome_command`](crate::App::on_chrome_command)
//! is then asked about each command the allowlist lets run, named by
//! [`ChromeCommand`], and may refuse it; it never sees, and so never allows,
//! a command Kurogane refuses.

use std::ffi::{CStr, c_int};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::OnceLock;

use tanso::*;

use tracing::{debug, error};

use crate::browser_registry::{BrowserId, BrowserType};
use crate::runtime::AppHandle;

/// A command Chromium runs in a window of the application's, from a key
/// shortcut or the context menu, as
/// [`App::on_chrome_command`](crate::App::on_chrome_command) is asked about
/// it. Chromium's own commands are folded by what the user asked for: every
/// kind of reload is [`Reload`](Self::Reload), the context menu's Copy and
/// Ctrl+C both [`Copy`](Self::Copy).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ChromeCommand {
    Back,
    Forward,
    /// Reload, also bypassing or clearing the cache.
    Reload,
    Stop,
    ZoomIn,
    ZoomOut,
    ZoomReset,
    Fullscreen,
    /// Find, find next and find previous.
    Find,
    Print,
    Cut,
    Copy,
    /// Paste, also matching the style around it.
    Paste,
    Delete,
    SelectAll,
    Undo,
    Redo,
    /// Closing the window (Ctrl+W and Ctrl+Shift+W).
    Close,
    /// DevTools, its console, inspect, and the context menu's Inspect.
    DevTools,
}

/// A command about to run, passed to
/// [`App::on_chrome_command`](crate::App::on_chrome_command).
#[derive(Debug, Clone)]
pub struct ChromeCommandRequest {
    command: ChromeCommand,
    browser: Option<BrowserId>,
}

impl ChromeCommandRequest {
    pub(crate) fn new(command: ChromeCommand, browser: Option<BrowserId>) -> Self {
        Self { command, browser }
    }

    /// The command.
    pub fn command(&self) -> ChromeCommand {
        self.command
    }

    /// The browser it runs in.
    pub fn browser(&self) -> Option<BrowserId> {
        self.browser
    }
}

/// The answer of [`App::on_chrome_command`](crate::App::on_chrome_command).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum CommandDecision {
    /// The command runs.
    #[default]
    Default,
    /// The command does not run.
    Refuse,
}

/// Strict allowlist of page-local commands, by their names in
/// `cef_command_ids.h`, with the command each is to the application.
const ALLOWED: &[(&CStr, ChromeCommand)] = &[
    // Navigation within the page
    (c"IDC_BACK", ChromeCommand::Back),
    (c"IDC_FORWARD", ChromeCommand::Forward),
    (c"IDC_RELOAD", ChromeCommand::Reload),
    (c"IDC_RELOAD_BYPASSING_CACHE", ChromeCommand::Reload),
    (c"IDC_RELOAD_CLEARING_CACHE", ChromeCommand::Reload),
    (c"IDC_STOP", ChromeCommand::Stop),
    // Zoom, fullscreen, find and print
    (c"IDC_ZOOM_PLUS", ChromeCommand::ZoomIn),
    (c"IDC_ZOOM_NORMAL", ChromeCommand::ZoomReset),
    (c"IDC_ZOOM_MINUS", ChromeCommand::ZoomOut),
    (c"IDC_FULLSCREEN", ChromeCommand::Fullscreen),
    (c"IDC_FIND", ChromeCommand::Find),
    (c"IDC_FIND_NEXT", ChromeCommand::Find),
    (c"IDC_FIND_PREVIOUS", ChromeCommand::Find),
    (c"IDC_PRINT", ChromeCommand::Print),
    // Editing, from shortcuts and from the context menu
    (c"IDC_CUT", ChromeCommand::Cut),
    (c"IDC_COPY", ChromeCommand::Copy),
    (c"IDC_PASTE", ChromeCommand::Paste),
    (c"IDC_CONTENT_CONTEXT_CUT", ChromeCommand::Cut),
    (c"IDC_CONTENT_CONTEXT_COPY", ChromeCommand::Copy),
    (c"IDC_CONTENT_CONTEXT_PASTE", ChromeCommand::Paste),
    (
        c"IDC_CONTENT_CONTEXT_PASTE_AND_MATCH_STYLE",
        ChromeCommand::Paste,
    ),
    (c"IDC_CONTENT_CONTEXT_DELETE", ChromeCommand::Delete),
    (c"IDC_CONTENT_CONTEXT_SELECTALL", ChromeCommand::SelectAll),
    (c"IDC_CONTENT_CONTEXT_UNDO", ChromeCommand::Undo),
    (c"IDC_CONTENT_CONTEXT_REDO", ChromeCommand::Redo),
    // Closing the window the command came from
    (c"IDC_CLOSE_TAB", ChromeCommand::Close),
    (c"IDC_CLOSE_WINDOW", ChromeCommand::Close),
    // Developer tools, which open as Kurogane popups
    (c"IDC_DEV_TOOLS", ChromeCommand::DevTools),
    (c"IDC_DEV_TOOLS_CONSOLE", ChromeCommand::DevTools),
    (c"IDC_DEV_TOOLS_INSPECT", ChromeCommand::DevTools),
    (c"IDC_DEV_TOOLS_TOGGLE", ChromeCommand::DevTools),
    (
        c"IDC_CONTENT_CONTEXT_INSPECTELEMENT",
        ChromeCommand::DevTools,
    ),
];

/// Resolves a stable command name to its dynamic runtime ID.
pub(crate) fn command_id(name: &CStr) -> Option<c_int> {
    // SAFETY: `name` is a valid, null-terminated C-string. The FFI boundary
    // guarantees read-only access and the backing memory outlives the call.
    let id = unsafe { tanso::sys::cef_id_for_command_id_name(name.as_ptr()) };
    (id >= 0).then_some(id)
}

/// Translates [`ALLOWED`] command names to runtime IDs.
/// Cached via [`OnceLock`] because CEF command IDs are unstable across Chromium
/// versions, whereas string names provide a stable resolution ABI.
fn allowed_ids() -> &'static [(c_int, ChromeCommand)] {
    static IDS: OnceLock<Vec<(c_int, ChromeCommand)>> = OnceLock::new();
    IDS.get_or_init(|| {
        ALLOWED
            .iter()
            .filter_map(|&(name, command)| Some((command_id(name)?, command)))
            .collect()
    })
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

/// What Kurogane does with the command `id`.
enum Verdict {
    /// An allowlisted command, which the application is asked about.
    Ask(ChromeCommand),
    /// A context menu item the application defined, which runs.
    Run,
    Refuse,
}

/// What a Kurogane window does with the command `id`.
fn verdict(id: c_int, disposition: WindowOpenDisposition) -> Verdict {
    if disposition != WindowOpenDisposition::CURRENT_TAB {
        return Verdict::Refuse;
    }
    if let Some(&(_, command)) = allowed_ids().iter().find(|(allowed, _)| *allowed == id) {
        return Verdict::Ask(command);
    }
    if custom_context_ids().is_some_and(|(first, last)| (first..=last).contains(&id)) {
        return Verdict::Run;
    }
    Verdict::Refuse
}

/// Whether the application refuses `command` in `browser`, from a key
/// shortcut or the context menu (`crate::context_menu`).
pub(crate) fn refuses(app: &AppHandle, command: ChromeCommand, browser: Option<BrowserId>) -> bool {
    decide(app, command, browser) == CommandDecision::Refuse
}

/// Asks the application's hook about `command` in `browser`. A hook that
/// panics refuses the command: the hook is there to restrict what runs.
fn decide(app: &AppHandle, command: ChromeCommand, browser: Option<BrowserId>) -> CommandDecision {
    let Some(hooks) = app.hooks() else {
        return CommandDecision::Default;
    };
    let Some(hook) = hooks.chrome_command.as_ref() else {
        return CommandDecision::Default;
    };
    // The guard ends with the statement
    let kind = browser.and_then(|id| {
        app.registry()
            .browsers
            .get(id)
            .map(|state| state.metadata.browser_type)
    });
    // Chromium's own browsers, DevTools' included, are not the application's
    if matches!(kind, Some(BrowserType::DevTools | BrowserType::ChromeUi)) {
        return CommandDecision::Default;
    }
    let request = ChromeCommandRequest::new(command, browser);
    match catch_unwind(AssertUnwindSafe(|| hook(&request, app))) {
        Ok(decision) => decision,
        Err(_) => {
            error!("on_chrome_command panicked; {command:?} is refused");
            CommandDecision::Refuse
        }
    }
}

// Leave CEF's four `is_chrome_*` callbacks at their defaults. cef-rs returns
// 0 for these callbacks, whereas CEF's C++ defaults return true. For a
// Chrome-style window, this keeps the app menu items, page actions and
// toolbar buttons hidden when Chromium creates them itself.
wrap_command_handler! {
    pub struct KuroganeCommandHandler {
        app: AppHandle,
    }

    impl CommandHandler {
        fn on_chrome_command(
            &self,
            browser: Option<&mut Browser>,
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

            match verdict(command_id, disposition) {
                Verdict::Ask(command) => match decide(
                    &self.app,
                    command,
                    browser.and_then(|browser| self.app.registry().browsers.find_id_by_browser(browser)),
                ) {
                    CommandDecision::Default => {
                        debug!("[Commands] {command:?} ({command_id}) runs");
                        0 // Unhandled: CEF runs the command
                    }
                    CommandDecision::Refuse => {
                        debug!("[Commands] {command:?} ({command_id}) refused by the application");
                        1
                    }
                },
                Verdict::Run => 0,
                Verdict::Refuse => {
                    debug!("[Commands] refused Chrome command {command_id} ({disposition:?})");
                    1 // Handled: CEF drops the command
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_allowed_command_has_one_name_and_every_command_is_reachable() {
        let mut names: Vec<&CStr> = ALLOWED.iter().map(|&(name, _)| name).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), ALLOWED.len(), "a name is listed twice");
        for command in [
            ChromeCommand::Back,
            ChromeCommand::Forward,
            ChromeCommand::Reload,
            ChromeCommand::Stop,
            ChromeCommand::ZoomIn,
            ChromeCommand::ZoomOut,
            ChromeCommand::ZoomReset,
            ChromeCommand::Fullscreen,
            ChromeCommand::Find,
            ChromeCommand::Print,
            ChromeCommand::Cut,
            ChromeCommand::Copy,
            ChromeCommand::Paste,
            ChromeCommand::Delete,
            ChromeCommand::SelectAll,
            ChromeCommand::Undo,
            ChromeCommand::Redo,
            ChromeCommand::Close,
            ChromeCommand::DevTools,
        ] {
            assert!(
                ALLOWED.iter().any(|&(_, allowed)| allowed == command),
                "{command:?} names no allowed command"
            );
        }
    }
}
