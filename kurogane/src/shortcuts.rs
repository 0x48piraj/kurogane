//! Kurogane's key shortcuts.
//!
//! One table per OS with the same commands on each. Closing the window and
//! quitting run before the page sees the key so that no page keeps them.
//! The page's commands (reload, find, print, zoom, back and forward,
//! fullscreen and DevTools) run once the page lets the key through. Each
//! runs as the Chromium command it names and asks `App::on_chrome_command`
//! as any other command does. Chromium's own shortcuts differ by OS and by
//! window state.
//!
//! Chromium's reserved keys for commands Kurogane refuses (new tabs and
//! windows, switching tabs) are ordinary keys the page gets first.

use std::ffi::CStr;

use crate::chrome_commands::{self, ChromeCommand};
use crate::keys::{Key, Modifiers, flag};

/// Kurogane shortcut of a key press.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Shortcut {
    /// Chromium command run before the page sees the key.
    BeforePage(&'static CStr),
    /// Chromium command run once the page lets the key through.
    AfterPage(&'static CStr),
    /// Key Chromium reserves for a command Kurogane refuses. The page gets it
    /// first as any key.
    Refused,
}

impl Shortcut {
    /// Returns the name of the Chromium command it runs.
    pub(crate) fn name(self) -> Option<&'static CStr> {
        match self {
            Self::BeforePage(name) | Self::AfterPage(name) => Some(name),
            Self::Refused => None,
        }
    }

    /// Returns the command it runs as `App::on_chrome_command` names it.
    /// macOS's Quit has none.
    pub(crate) fn command(self) -> Option<ChromeCommand> {
        self.name().and_then(chrome_commands::command_named)
    }
}

/// Returns the shortcut of `key` pressed with exactly `modifiers`.
pub(crate) fn shortcut_for(key: Key, modifiers: Modifiers) -> Option<Shortcut> {
    let chord = modifiers.chord();
    let named = |table: &[(Key, u32, &'static CStr)]| {
        table
            .iter()
            .find(|&&(k, m, _)| k == key && m == chord)
            .map(|&(_, _, name)| name)
    };
    if let Some(name) = named(BEFORE_PAGE) {
        return Some(Shortcut::BeforePage(name));
    }
    if let Some(name) = named(AFTER_PAGE) {
        return Some(Shortcut::AfterPage(name));
    }
    REFUSED
        .iter()
        .any(|&(k, m)| k == key && m == chord)
        .then_some(Shortcut::Refused)
}

const NONE: u32 = 0;
const SHIFT: u32 = flag::SHIFT_DOWN;
const CTRL: u32 = flag::CONTROL_DOWN;
const ALT: u32 = flag::ALT_DOWN;
#[cfg(target_os = "macos")]
const CMD: u32 = flag::COMMAND_DOWN;

// Keys by the Windows virtual-key code CEF reports
const PLUS: Key = Key::Other(0xBB);
const MINUS: Key = Key::Other(0xBD);
#[cfg(not(target_os = "macos"))]
const PAD_PLUS: Key = Key::Other(0x6B);
#[cfg(not(target_os = "macos"))]
const PAD_MINUS: Key = Key::Other(0x6D);
#[cfg(not(target_os = "macos"))]
const PAD_0: Key = Key::Other(0x60);
#[cfg(target_os = "macos")]
const PERIOD: Key = Key::Other(0xBE);
#[cfg(target_os = "macos")]
const LEFT_BRACKET: Key = Key::Other(0xDB);
#[cfg(target_os = "macos")]
const RIGHT_BRACKET: Key = Key::Other(0xDD);

/// Shortcuts run before the page. These are Close and macOS's Quit.
#[cfg(not(target_os = "macos"))]
const BEFORE_PAGE: &[(Key, u32, &CStr)] = &[
    (Key::Char('W'), CTRL, c"IDC_CLOSE_TAB"),
    (Key::F(4), CTRL, c"IDC_CLOSE_TAB"),
    (Key::Char('W'), CTRL | SHIFT, c"IDC_CLOSE_WINDOW"),
    (Key::F(4), ALT, c"IDC_CLOSE_WINDOW"),
];
#[cfg(target_os = "macos")]
const BEFORE_PAGE: &[(Key, u32, &CStr)] = &[
    (Key::Char('W'), CMD, c"IDC_CLOSE_TAB"),
    (Key::Char('W'), CMD | SHIFT, c"IDC_CLOSE_WINDOW"),
    (Key::Char('Q'), CMD, c"IDC_EXIT"),
];

/// Shortcuts run once the page lets the key through. Their keys are
/// Chromium's on Windows and Linux and Chrome's main menu's on macOS.
#[cfg(not(target_os = "macos"))]
const AFTER_PAGE: &[(Key, u32, &CStr)] = &[
    (Key::Char('R'), CTRL, c"IDC_RELOAD"),
    (Key::F(5), NONE, c"IDC_RELOAD"),
    (Key::Char('R'), CTRL | SHIFT, c"IDC_RELOAD_BYPASSING_CACHE"),
    (Key::F(5), CTRL, c"IDC_RELOAD_BYPASSING_CACHE"),
    (Key::F(5), SHIFT, c"IDC_RELOAD_BYPASSING_CACHE"),
    (Key::Char('F'), CTRL, c"IDC_FIND"),
    (Key::Char('G'), CTRL, c"IDC_FIND_NEXT"),
    (Key::F(3), NONE, c"IDC_FIND_NEXT"),
    (Key::Char('G'), CTRL | SHIFT, c"IDC_FIND_PREVIOUS"),
    (Key::F(3), SHIFT, c"IDC_FIND_PREVIOUS"),
    (Key::Char('P'), CTRL, c"IDC_PRINT"),
    (PLUS, CTRL, c"IDC_ZOOM_PLUS"),
    (PLUS, CTRL | SHIFT, c"IDC_ZOOM_PLUS"),
    (PAD_PLUS, CTRL, c"IDC_ZOOM_PLUS"),
    (MINUS, CTRL, c"IDC_ZOOM_MINUS"),
    (MINUS, CTRL | SHIFT, c"IDC_ZOOM_MINUS"),
    (PAD_MINUS, CTRL, c"IDC_ZOOM_MINUS"),
    (Key::Digit(0), CTRL, c"IDC_ZOOM_NORMAL"),
    (PAD_0, CTRL, c"IDC_ZOOM_NORMAL"),
    (Key::ArrowLeft, ALT, c"IDC_BACK"),
    (Key::ArrowRight, ALT, c"IDC_FORWARD"),
    (Key::F(11), NONE, c"IDC_FULLSCREEN"),
    (Key::F(12), NONE, c"IDC_DEV_TOOLS_TOGGLE"),
    (Key::Char('I'), CTRL | SHIFT, c"IDC_DEV_TOOLS"),
    (Key::Char('J'), CTRL | SHIFT, c"IDC_DEV_TOOLS_CONSOLE"),
    (Key::Char('C'), CTRL | SHIFT, c"IDC_DEV_TOOLS_INSPECT"),
];
#[cfg(target_os = "macos")]
const AFTER_PAGE: &[(Key, u32, &CStr)] = &[
    (Key::Char('R'), CMD, c"IDC_RELOAD"),
    (Key::Char('R'), CMD | SHIFT, c"IDC_RELOAD_BYPASSING_CACHE"),
    (PERIOD, CMD, c"IDC_STOP"),
    (Key::Char('F'), CMD, c"IDC_FIND"),
    (Key::Char('G'), CMD, c"IDC_FIND_NEXT"),
    (Key::Char('G'), CMD | SHIFT, c"IDC_FIND_PREVIOUS"),
    (Key::Char('P'), CMD, c"IDC_PRINT"),
    (PLUS, CMD, c"IDC_ZOOM_PLUS"),
    (PLUS, CMD | SHIFT, c"IDC_ZOOM_PLUS"),
    (MINUS, CMD, c"IDC_ZOOM_MINUS"),
    (Key::Digit(0), CMD, c"IDC_ZOOM_NORMAL"),
    (LEFT_BRACKET, CMD, c"IDC_BACK"),
    (RIGHT_BRACKET, CMD, c"IDC_FORWARD"),
    (Key::Char('F'), CMD | CTRL, c"IDC_FULLSCREEN"),
    (Key::F(12), NONE, c"IDC_DEV_TOOLS_TOGGLE"),
    (Key::Char('I'), CMD | ALT, c"IDC_DEV_TOOLS"),
    (Key::Char('J'), CMD | ALT, c"IDC_DEV_TOOLS_CONSOLE"),
    (Key::Char('C'), CMD | ALT, c"IDC_DEV_TOOLS_INSPECT"),
];

/// Keys Chromium reserves on Windows and Linux for commands Kurogane refuses
/// (`BrowserCommandController::IsReservedCommandOrKey`). macOS has them only
/// in Chrome's main menu.
#[cfg(not(target_os = "macos"))]
const REFUSED: &[(Key, u32)] = &[
    (Key::Char('T'), CTRL),
    (Key::Char('N'), CTRL),
    (Key::Char('N'), CTRL | SHIFT),
    (Key::Char('T'), CTRL | SHIFT),
    (Key::PageDown, CTRL),
    (Key::PageUp, CTRL),
];
#[cfg(target_os = "macos")]
const REFUSED: &[(Key, u32)] = &[];

#[cfg(test)]
mod tests {
    use super::*;

    /// Returns the shortcut of the key of virtual-key code `key` pressed with
    /// `modifiers`.
    fn shortcut(key: u32, modifiers: u32) -> Option<Shortcut> {
        let press = crate::keys::KeyPress::new(key, modifiers, 0, false, None);
        shortcut_for(press.key(), press.modifiers())
    }

    #[test]
    fn every_shortcut_is_a_command_the_application_is_asked_about() {
        for &(key, modifiers, name) in BEFORE_PAGE.iter().chain(AFTER_PAGE) {
            let quit = cfg!(target_os = "macos") && name == c"IDC_EXIT";
            assert!(
                quit || chrome_commands::command_named(name).is_some(),
                "{key:?} {modifiers:#x} runs {name:?}, which Kurogane refuses"
            );
        }
    }

    #[test]
    fn a_key_is_one_shortcut_at_most() {
        let all: Vec<(Key, u32)> = BEFORE_PAGE
            .iter()
            .chain(AFTER_PAGE)
            .map(|&(k, m, _)| (k, m))
            .chain(REFUSED.iter().copied())
            .collect();
        for (i, chord) in all.iter().enumerate() {
            assert!(!all[i + 1..].contains(chord), "{chord:?} is listed twice");
        }
    }

    #[test]
    fn closing_runs_before_the_page_and_the_page_s_commands_after() {
        let primary = if cfg!(target_os = "macos") {
            flag::COMMAND_DOWN
        } else {
            CTRL
        };
        let close = shortcut(0x57, primary);
        assert_eq!(close, Some(Shortcut::BeforePage(c"IDC_CLOSE_TAB")));
        assert_eq!(
            close.and_then(Shortcut::command),
            Some(ChromeCommand::Close)
        );
        assert_eq!(
            shortcut(0x52, primary).and_then(Shortcut::command),
            Some(ChromeCommand::Reload)
        );
        assert!(matches!(
            shortcut(0x46, primary),
            Some(Shortcut::AfterPage(_))
        ));
        // AltGr arrives as Ctrl+Alt on Windows and makes no shortcut
        assert_eq!(shortcut(0x57, primary | ALT), None);
        assert_eq!(shortcut(0x57, NONE), None);
        assert_eq!(shortcut(0x41, primary), None);
    }

    #[test]
    fn devtools_has_the_platform_s_keys() {
        let devtools = if cfg!(target_os = "macos") {
            shortcut(0x49, flag::COMMAND_DOWN | ALT)
        } else {
            shortcut(0x49, CTRL | SHIFT)
        };
        assert_eq!(
            devtools.and_then(Shortcut::command),
            Some(ChromeCommand::DevTools)
        );
        assert_eq!(
            shortcut(0x7B, NONE).and_then(Shortcut::command),
            Some(ChromeCommand::DevTools)
        );
    }

    #[test]
    fn chromium_s_tab_and_window_keys_are_refused_where_chromium_reserves_them() {
        let new_tab = shortcut(0x54, CTRL);
        if cfg!(target_os = "macos") {
            assert_eq!(new_tab, None);
            assert_eq!(
                shortcut(0x51, flag::COMMAND_DOWN),
                Some(Shortcut::BeforePage(c"IDC_EXIT"))
            );
            assert_eq!(Shortcut::BeforePage(c"IDC_EXIT").command(), None);
        } else {
            assert_eq!(new_tab, Some(Shortcut::Refused));
            assert_eq!(shortcut(0x22, CTRL), Some(Shortcut::Refused));
            assert_eq!(Shortcut::Refused.name(), None);
        }
    }
}
