//! Key presses in a window of the application's.
//!
//! [`App::on_key`](crate::App::on_key) is asked about each key press and its
//! repeats. It never sees a release or the character a key types. Its
//! [`KeyDecision`] sends the key on in Kurogane's order, nowhere or to the
//! page first. Kurogane's shortcuts (crate::shortcuts) are the same on every
//! OS.

use std::ffi::CStr;
use std::panic::{AssertUnwindSafe, catch_unwind};

use tracing::{debug, error};

use crate::browser_registry::BrowserId;
use crate::chrome_commands::ChromeCommand;
use crate::runtime::AppHandle;
use crate::shortcuts::Shortcut;

/// A key the user pressed, passed to [`App::on_key`](crate::App::on_key).
#[derive(Debug, Clone)]
pub struct KeyPress {
    key: Key,
    modifiers: Modifiers,
    character: Option<char>,
    in_editable_field: bool,
    browser: Option<BrowserId>,
    command: Option<ChromeCommand>,
}

impl KeyPress {
    /// The press of the key of Windows virtual-key code `code` (CEF reports
    /// keys by that code on every platform), with CEF's event flags
    /// `flags`, typing `character`.
    pub(crate) fn new(
        code: u32,
        flags: u32,
        character: u16,
        in_editable_field: bool,
        browser: Option<BrowserId>,
    ) -> Self {
        Self {
            key: Key::from_code(code),
            modifiers: Modifiers(flags),
            character: char::from_u32(u32::from(character)).filter(|c| !c.is_control()),
            in_editable_field,
            browser,
            command: None,
        }
    }

    /// Returns the press with `command` as its shortcut's command.
    pub(crate) fn with_command(self, command: Option<ChromeCommand>) -> Self {
        Self { command, ..self }
    }

    /// The key.
    pub fn key(&self) -> Key {
        self.key
    }

    /// The modifier keys held with it.
    pub fn modifiers(&self) -> Modifiers {
        self.modifiers
    }

    /// Returns the character the press types. There is none with a modifier
    /// that turns it into a control character (Ctrl+W) and none for keys
    /// that type nothing (F5, the arrows).
    pub fn character(&self) -> Option<char> {
        self.character
    }

    /// Whether the focus is in a field the user types into (an input, a
    /// text area, an editable element).
    pub fn in_editable_field(&self) -> bool {
        self.in_editable_field
    }

    /// Whether the key is held down and this press is its repeat.
    pub fn repeat(&self) -> bool {
        self.modifiers.has(flag::IS_REPEAT)
    }

    /// The browser the key went to.
    pub fn browser(&self) -> Option<BrowserId> {
        self.browser
    }

    /// Returns the command the key runs as one of Kurogane's shortcuts on
    /// any OS. Ctrl+W (Cmd+W on macOS) is [`Close`](ChromeCommand::Close).
    /// Ctrl+R and F5 are [`Reload`](ChromeCommand::Reload). Ctrl+Shift+I
    /// (Cmd+Option+I) and F12 are [`DevTools`](ChromeCommand::DevTools).
    /// macOS's Cmd+Q quits and names no command. A browser embedded in the
    /// application's own window runs no shortcut and names none.
    pub fn command(&self) -> Option<ChromeCommand> {
        self.command
    }
}

/// A key by its place on the keyboard rather than what it types. Shift+W
/// and W are both `Key::Char('W')`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Key {
    /// A letter key, as its uppercase ASCII letter.
    Char(char),
    /// A digit key of the top row, 0 to 9.
    Digit(u8),
    /// A function key, F1 to F24.
    F(u8),
    Escape,
    Enter,
    Tab,
    Space,
    Backspace,
    Delete,
    Insert,
    Home,
    End,
    PageUp,
    PageDown,
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    /// Any other key by its Windows virtual-key code, which CEF uses on
    /// every platform. Such keys are punctuation (whose code depends on the
    /// keyboard layout), the keypad and the modifier keys themselves.
    Other(u32),
}

impl Key {
    /// The key of Windows virtual-key code `code`.
    fn from_code(code: u32) -> Key {
        match code {
            0x41..=0x5A => Key::Char(char::from(code as u8)),
            0x30..=0x39 => Key::Digit((code - 0x30) as u8),
            0x70..=0x87 => Key::F((code - 0x70 + 1) as u8),
            0x1B => Key::Escape,
            0x0D => Key::Enter,
            0x09 => Key::Tab,
            0x20 => Key::Space,
            0x08 => Key::Backspace,
            0x2E => Key::Delete,
            0x2D => Key::Insert,
            0x24 => Key::Home,
            0x23 => Key::End,
            0x21 => Key::PageUp,
            0x22 => Key::PageDown,
            0x26 => Key::ArrowUp,
            0x28 => Key::ArrowDown,
            0x25 => Key::ArrowLeft,
            0x27 => Key::ArrowRight,
            other => Key::Other(other),
        }
    }
}

/// CEF's event flags this module reads (cef_types.h, cef_event_flags_t), as
/// the `u32` a key event carries them in. tanso's own constants wrap a C
/// `int` on Windows and an `unsigned int` elsewhere; a test holds these to
/// them on every platform.
pub(crate) mod flag {
    pub const SHIFT_DOWN: u32 = 1 << 1;
    pub const CONTROL_DOWN: u32 = 1 << 2;
    pub const ALT_DOWN: u32 = 1 << 3;
    pub const COMMAND_DOWN: u32 = 1 << 7;
    pub const IS_REPEAT: u32 = 1 << 13;
}

/// The modifier keys held with a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Modifiers(u32);

impl Modifiers {
    fn has(self, flag: u32) -> bool {
        self.0 & flag != 0
    }

    pub fn shift(self) -> bool {
        self.has(flag::SHIFT_DOWN)
    }

    pub fn ctrl(self) -> bool {
        self.has(flag::CONTROL_DOWN)
    }

    /// Alt, Option on macOS.
    pub fn alt(self) -> bool {
        self.has(flag::ALT_DOWN)
    }

    /// Cmd on macOS, the Windows key elsewhere.
    pub fn meta(self) -> bool {
        self.has(flag::COMMAND_DOWN)
    }

    /// Returns whether the modifier of the platform's shortcuts is held (Cmd
    /// on macOS, Ctrl elsewhere). Ctrl+W on Windows and Linux and Cmd+W on
    /// macOS both have it.
    pub fn primary(self) -> bool {
        if cfg!(target_os = "macos") {
            self.meta()
        } else {
            self.ctrl()
        }
    }

    /// Whether no modifier is held (Shift included).
    pub fn none(self) -> bool {
        !(self.shift() || self.ctrl() || self.alt() || self.meta())
    }

    /// Returns CEF's flags of the modifier keys held and no other flag.
    pub(crate) fn chord(self) -> u32 {
        self.0 & (flag::SHIFT_DOWN | flag::CONTROL_DOWN | flag::ALT_DOWN | flag::COMMAND_DOWN)
    }
}

/// The answer of [`App::on_key`](crate::App::on_key).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum KeyDecision {
    /// The key goes on in Kurogane's order on every OS. A shortcut that
    /// closes the window or quits (Ctrl+W, Cmd+Q on macOS) runs before the
    /// page sees the key. The page never gets it. Every other shortcut
    /// (Ctrl+F, Ctrl+P, Ctrl+R, F5) reaches the page first and runs only if
    /// the page does not prevent the key's default. Kurogane refuses the
    /// commands of Chromium's keys for new tabs and windows (Ctrl+T,
    /// Ctrl+N). Those keys reach the page as any key.
    #[default]
    Default,
    /// The key goes nowhere. Neither a shortcut nor the page sees it, its
    /// character or its release.
    Consume,
    /// The key goes to the page first. Its shortcut runs only if the page
    /// does not prevent the key's default. Under [`Default`](Self::Default)
    /// every shortcut but closing and quitting has this order. A page binds
    /// Ctrl+W for itself this way.
    ///
    /// For shortcuts only. A key that types a character and is answered
    /// `PageFirst` types nothing unless the page handles its keydown.
    /// Chromium drops the character of a shortcut the page left alone.
    /// AltGr arrives as Ctrl+Alt on Windows.
    ///
    /// The page then decides whether the shortcut runs. One that prevents
    /// Ctrl+W keeps its window open and one that hangs holds the key.
    /// Closing and quitting run before the page so that no page can keep
    /// them. Answer `PageFirst` only for windows that show the application's
    /// own pages ([`KeyPress::browser`]).
    PageFirst,
}

/// Destination of a key on its way to a page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Route {
    /// Nowhere. Neither the page nor Chromium sees it.
    Taken,
    /// Chromium command Kurogane runs in Chromium's place. The key goes
    /// nowhere else.
    Run(&'static CStr),
    /// Page first. Chromium holds the key as a shortcut until the page lets
    /// it through.
    PageFirst,
    /// Chromium's own order.
    On,
}

/// Returns where a key goes before the page sees it for the application's
/// `decision` and the key's `shortcut`.
pub(crate) fn before_page(decision: KeyDecision, shortcut: Option<Shortcut>) -> Route {
    match (decision, shortcut) {
        (KeyDecision::Consume, _) => Route::Taken,
        (KeyDecision::PageFirst, _) => Route::PageFirst,
        (KeyDecision::Default, Some(Shortcut::BeforePage(name))) => Route::Run(name),
        (KeyDecision::Default, Some(Shortcut::AfterPage(_) | Shortcut::Refused)) => {
            Route::PageFirst
        }
        (KeyDecision::Default, None) => Route::On,
    }
}

/// Asks the application's hook about `press`. Runs on CEF's UI thread for
/// every key press; the hook must be quick. A hook that panics lets the key
/// through. Swallowing keys would take the keyboard from the user.
pub(crate) fn decide(app: &AppHandle, press: &KeyPress) -> KeyDecision {
    let Some(hooks) = app.hooks() else {
        return KeyDecision::Default;
    };
    let Some(hook) = hooks.key.as_ref() else {
        return KeyDecision::Default;
    };
    match catch_unwind(AssertUnwindSafe(|| hook(press, app))) {
        Ok(decision) => {
            if decision == KeyDecision::Consume {
                debug!("key {:?} consumed by the application", press.key);
            }
            decision
        }
        Err(_) => {
            error!("on_key panicked; the key {:?} goes on", press.key);
            KeyDecision::Default
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_flags_are_cef_s() {
        use tanso::sys::cef_event_flags_t as Flags;
        for (ours, cef) in [
            (flag::SHIFT_DOWN, Flags::EVENTFLAG_SHIFT_DOWN),
            (flag::CONTROL_DOWN, Flags::EVENTFLAG_CONTROL_DOWN),
            (flag::ALT_DOWN, Flags::EVENTFLAG_ALT_DOWN),
            (flag::COMMAND_DOWN, Flags::EVENTFLAG_COMMAND_DOWN),
            (flag::IS_REPEAT, Flags::EVENTFLAG_IS_REPEAT),
        ] {
            assert_eq!(i64::from(ours), i64::from(cef.0), "{cef:?}");
        }
    }

    #[test]
    fn keys_are_named_by_place_and_the_rest_by_code() {
        for (code, key) in [
            (0x57, Key::Char('W')),
            (0x41, Key::Char('A')),
            (0x30, Key::Digit(0)),
            (0x39, Key::Digit(9)),
            (0x70, Key::F(1)),
            (0x74, Key::F(5)),
            (0x87, Key::F(24)),
            (0x1B, Key::Escape),
            (0x0D, Key::Enter),
            (0x25, Key::ArrowLeft),
            (0x2E, Key::Delete),
            // Ctrl itself, a keypad digit and a layout-dependent key
            (0x11, Key::Other(0x11)),
            (0x60, Key::Other(0x60)),
            (0xBB, Key::Other(0xBB)),
        ] {
            assert_eq!(Key::from_code(code), key, "{code:#x}");
        }
    }

    #[test]
    fn the_primary_modifier_is_the_platform_s_and_control_characters_are_none() {
        let ctrl_w = KeyPress::new(0x57, flag::CONTROL_DOWN, 0x17, false, None);
        let cmd_w = KeyPress::new(0x57, flag::COMMAND_DOWN, 0x77, false, None);
        assert_eq!(ctrl_w.modifiers().primary(), !cfg!(target_os = "macos"));
        assert_eq!(cmd_w.modifiers().primary(), cfg!(target_os = "macos"));
        assert_eq!(ctrl_w.character(), None);

        let shift_w = KeyPress::new(0x57, flag::SHIFT_DOWN, u16::from(b'W'), true, None);
        assert_eq!(shift_w.key(), Key::Char('W'));
        assert_eq!(shift_w.character(), Some('W'));
        assert!(shift_w.modifiers().shift() && !shift_w.modifiers().none());
        assert!(shift_w.in_editable_field());

        let held = KeyPress::new(0x28, flag::IS_REPEAT, 0, false, None);
        assert!(held.repeat() && held.modifiers().none());
        assert_eq!(held.modifiers().chord(), 0);
        assert_eq!(held.command(), None);
    }

    #[test]
    fn closing_runs_before_the_page_unless_the_application_says_otherwise() {
        let close = Some(Shortcut::BeforePage(c"IDC_CLOSE_TAB"));
        let find = Some(Shortcut::AfterPage(c"IDC_FIND"));
        for (decision, shortcut, route) in [
            (KeyDecision::Default, close, Route::Run(c"IDC_CLOSE_TAB")),
            (KeyDecision::PageFirst, close, Route::PageFirst),
            (KeyDecision::Consume, close, Route::Taken),
            (KeyDecision::Default, find, Route::PageFirst),
            (KeyDecision::Consume, find, Route::Taken),
            // The page gets a refused key first as any key
            (
                KeyDecision::Default,
                Some(Shortcut::Refused),
                Route::PageFirst,
            ),
            (KeyDecision::Default, None, Route::On),
            (KeyDecision::PageFirst, None, Route::PageFirst),
            (KeyDecision::Consume, None, Route::Taken),
        ] {
            assert_eq!(
                before_page(decision, shortcut),
                route,
                "{decision:?} {shortcut:?}"
            );
        }
    }
}
