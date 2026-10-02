//! The keys a user presses in a window of the application's, before the page
//! or Chromium's own shortcuts see them.
//!
//! [`App::on_key`](crate::App::on_key) is asked about each key press (the
//! key going down, repeats included; never its release or the character it
//! types) and may consume it: then neither the page nor Chromium's
//! shortcuts (Ctrl+R, Ctrl+W, F5, Ctrl+Shift+I, ...) see the key, nor its
//! character or release.

use std::panic::{AssertUnwindSafe, catch_unwind};

use tracing::{debug, error};

use crate::browser_registry::BrowserId;
use crate::runtime::AppHandle;

/// A key the user pressed, passed to [`App::on_key`](crate::App::on_key).
#[derive(Debug, Clone)]
pub struct KeyPress {
    key: Key,
    modifiers: Modifiers,
    character: Option<char>,
    in_editable_field: bool,
    browser: Option<BrowserId>,
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
        }
    }

    /// The key.
    pub fn key(&self) -> Key {
        self.key
    }

    /// The modifier keys held with it.
    pub fn modifiers(&self) -> Modifiers {
        self.modifiers
    }

    /// The character the press types, if any: none with a modifier that
    /// turns it into a control character (Ctrl+W), none for keys that type
    /// nothing (F5, the arrows).
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
}

/// A key, by where it is on the keyboard rather than what it types: Shift+W
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
    /// Any other key, by its Windows virtual-key code, which CEF uses on
    /// every platform: punctuation, whose code depends on the keyboard
    /// layout, the keypad, the modifier keys themselves.
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
/// the `u32` a key event carries them in. cef-rs's own constants wrap a C
/// `int` on Windows and an `unsigned int` elsewhere; a test holds these to
/// them on every platform.
mod flag {
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

    /// The modifier of the platform's shortcuts: Cmd on macOS, Ctrl
    /// elsewhere. Ctrl+W on Windows and Linux and Cmd+W on macOS both have
    /// it.
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
}

/// The answer of [`App::on_key`](crate::App::on_key).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum KeyDecision {
    /// The key goes on: to Chromium's shortcuts, then the page.
    #[default]
    Default,
    /// The key goes nowhere: neither Chromium's shortcuts nor the page see
    /// it, its character or its release.
    Consume,
}

/// Asks the application's hook about `press`. Runs on CEF's UI thread for
/// every key press, so the hook must be quick. A hook that panics lets the
/// key through: swallowing keys would take the keyboard from the user.
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
        use cef::sys::cef_event_flags_t as Flags;
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
    }
}
