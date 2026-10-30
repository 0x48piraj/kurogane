//! Key presses in a browser of the application's.
//!
//! [`App::on_key`](crate::App::on_key) is asked about each key press and its
//! repeats. It never sees a release. Its [`KeyDecision`] sends the key on in
//! Kurogane's order, nowhere or to the page first. Kurogane's shortcuts
//! (crate::shortcuts) are the same on every OS.

use std::ffi::CStr;
use std::ops::{BitOr, BitOrAssign};
use std::panic::{AssertUnwindSafe, catch_unwind};

use tanso::KeyEvent;
use tracing::{debug, error};

use crate::browser_registry::BrowserId;
use crate::chrome_commands::ChromeCommand;
use crate::keyboard::{LogicalKey, PhysicalCode, us_characters};
use crate::runtime::AppHandle;
use crate::shortcuts::Shortcut;

/// A key the user pressed, passed to [`App::on_key`](crate::App::on_key).
#[derive(Debug, Clone)]
pub struct KeyPress {
    code: PhysicalCode,
    key: LogicalKey,
    text: Option<String>,
    /// Windows virtual-key code CEF reports on every OS, which Chromium's
    /// shortcuts match on
    virtual_key: u32,
    modifiers: Modifiers,
    in_editable_field: bool,
    browser: Option<BrowserId>,
    command: Option<ChromeCommand>,
}

impl KeyPress {
    /// The press CEF reports as `event` in `browser`.
    pub(crate) fn from_cef(event: &KeyEvent, browser: Option<BrowserId>) -> Self {
        let printable = |unit: u16| char::from_u32(u32::from(unit)).filter(|c| !c.is_control());
        let virtual_key = u32::try_from(event.windows_key_code).unwrap_or(0);
        let modifiers = Modifiers(event.modifiers);
        let key = printable(event.character)
            .or_else(|| printable(event.unmodified_character))
            .map(|c| LogicalKey::Character(c.to_string()))
            .or_else(|| LogicalKey::from_virtual_key(virtual_key))
            .or_else(|| {
                without_ctrl(event.unmodified_character, virtual_key, modifiers)
                    .map(|c| LogicalKey::Character(c.to_string()))
            })
            .unwrap_or(LogicalKey::Unidentified);
        Self {
            code: physical_code(event.native_key_code),
            key,
            text: printable(event.character).map(String::from),
            virtual_key,
            modifiers,
            in_editable_field: event.focus_on_editable_field != 0,
            browser,
            command: None,
        }
    }

    /// Returns the press with `command` as its shortcut's command.
    pub(crate) fn with_command(self, command: Option<ChromeCommand>) -> Self {
        Self { command, ..self }
    }

    /// The key's place on the keyboard, the same in every layout.
    pub fn code(&self) -> PhysicalCode {
        self.code
    }

    /// What the key means in the layout, the character it types without
    /// Ctrl or Cmd or a named key. Shift+W is `"W"` and Ctrl+W `"w"`.
    pub fn key(&self) -> &LogicalKey {
        &self.key
    }

    /// Windows virtual-key code of the key, as Chromium's shortcuts know it.
    pub(crate) fn virtual_key(&self) -> u32 {
        self.virtual_key
    }

    /// The modifier keys held with it.
    pub fn modifiers(&self) -> Modifiers {
        self.modifiers
    }

    /// Returns the text the press types. There is none with a modifier that
    /// turns it into a control character (Ctrl+W) and none for keys that
    /// type nothing (F5, the arrows).
    pub fn text(&self) -> Option<&str> {
        self.text.as_deref()
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

/// Returns the key of the native code CEF reports, `Unidentified` for one
/// Chromium names no key for. On Windows a value above 0xffff is the lParam
/// of a native key message (the scan code in bits 16 to 23, bit 24 for an
/// extended key) and any other the scan code itself, 0xe000 for an extended
/// key, as Chromium gives it for an event of no native message.
fn physical_code(native: i32) -> PhysicalCode {
    #[cfg(target_os = "windows")]
    let code = {
        let native = native.cast_unsigned();
        let scan_code = if native > 0xffff {
            let extended = if native & (1 << 24) != 0 { 0xe000 } else { 0 };
            ((native >> 16) & 0xff) as u16 | extended
        } else {
            native as u16
        };
        PhysicalCode::from_windows_scan_code(scan_code)
    };
    #[cfg(target_os = "linux")]
    let code = u16::try_from(native)
        .ok()
        .and_then(PhysicalCode::from_xkb_keycode);
    #[cfg(target_os = "macos")]
    let code = u16::try_from(native)
        .ok()
        .and_then(PhysicalCode::from_mac_keycode);
    code.unwrap_or(PhysicalCode::Unidentified)
}

/// Returns the character a Ctrl chord types without Ctrl. Chromium reports
/// the chord's character as a control character, the character's low five
/// bits. The virtual key's US character settles it, else the one other
/// character that fits and is no letter.
fn without_ctrl(control: u16, virtual_key: u32, modifiers: Modifiers) -> Option<char> {
    let [unshifted, shifted] = us_characters(virtual_key)?;
    if !modifiers.ctrl() || control >= 0x20 {
        return None;
    }
    let fits = |c: char| matches!(c, ' '..='~') && u32::from(c) & 0x1f == u32::from(control);

    // CapsLock shifts letters only
    let shift = if unshifted.is_ascii_alphabetic() {
        modifiers.shift() != modifiers.has(flag::CAPS_LOCK_ON)
    } else {
        modifiers.shift()
    };
    let us = if shift { shifted } else { unshifted };
    if fits(us) {
        return Some(us);
    }
    let mut others = [0x20, 0x40, 0x60]
        .into_iter()
        .filter_map(|bits| char::from_u32(u32::from(control) | bits))
        .filter(|&c| fits(c) && !c.is_ascii_alphabetic());
    match (others.next(), others.next()) {
        (Some(c), None) => Some(c),
        _ => None,
    }
}

/// CEF's event flags this module reads (cef_types.h, cef_event_flags_t), as
/// the `u32` a key event carries them in. tanso's own constants wrap a C
/// `int` on Windows and an `unsigned int` elsewhere; a test holds these to
/// them on every platform.
pub(crate) mod flag {
    pub const CAPS_LOCK_ON: u32 = 1 << 0;
    pub const SHIFT_DOWN: u32 = 1 << 1;
    pub const CONTROL_DOWN: u32 = 1 << 2;
    pub const ALT_DOWN: u32 = 1 << 3;
    pub const LEFT_MOUSE_BUTTON: u32 = 1 << 4;
    pub const MIDDLE_MOUSE_BUTTON: u32 = 1 << 5;
    pub const RIGHT_MOUSE_BUTTON: u32 = 1 << 6;
    pub const COMMAND_DOWN: u32 = 1 << 7;
    pub const NUM_LOCK_ON: u32 = 1 << 8;
    pub const IS_KEY_PAD: u32 = 1 << 9;
    pub const IS_LEFT: u32 = 1 << 10;
    pub const IS_RIGHT: u32 = 1 << 11;
    pub const ALTGR_DOWN: u32 = 1 << 12;
    pub const IS_REPEAT: u32 = 1 << 13;
}

/// Modifier keys held and locks on with a key or an offscreen browser's
/// input. The constants combine with `|`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Modifiers(u32);

impl Modifiers {
    pub const NONE: Self = Self(0);
    pub const SHIFT: Self = Self(flag::SHIFT_DOWN);
    pub const CONTROL: Self = Self(flag::CONTROL_DOWN);
    /// Alt, Option on macOS.
    pub const ALT: Self = Self(flag::ALT_DOWN);
    /// Cmd on macOS, the Windows key elsewhere.
    pub const META: Self = Self(flag::COMMAND_DOWN);
    pub const CAPS_LOCK: Self = Self(flag::CAPS_LOCK_ON);
    pub const NUM_LOCK: Self = Self(flag::NUM_LOCK_ON);

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

    /// Returns CEF's flags of the modifier keys held and the locks on.
    pub(crate) fn state(self) -> u32 {
        self.chord() | self.0 & (flag::CAPS_LOCK_ON | flag::NUM_LOCK_ON)
    }
}

impl BitOr for Modifiers {
    type Output = Self;

    fn bitor(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

impl BitOrAssign for Modifiers {
    fn bitor_assign(&mut self, other: Self) {
        self.0 |= other.0;
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
            (flag::CAPS_LOCK_ON, Flags::EVENTFLAG_CAPS_LOCK_ON),
            (flag::SHIFT_DOWN, Flags::EVENTFLAG_SHIFT_DOWN),
            (flag::CONTROL_DOWN, Flags::EVENTFLAG_CONTROL_DOWN),
            (flag::ALT_DOWN, Flags::EVENTFLAG_ALT_DOWN),
            (flag::LEFT_MOUSE_BUTTON, Flags::EVENTFLAG_LEFT_MOUSE_BUTTON),
            (
                flag::MIDDLE_MOUSE_BUTTON,
                Flags::EVENTFLAG_MIDDLE_MOUSE_BUTTON,
            ),
            (
                flag::RIGHT_MOUSE_BUTTON,
                Flags::EVENTFLAG_RIGHT_MOUSE_BUTTON,
            ),
            (flag::COMMAND_DOWN, Flags::EVENTFLAG_COMMAND_DOWN),
            (flag::NUM_LOCK_ON, Flags::EVENTFLAG_NUM_LOCK_ON),
            (flag::IS_KEY_PAD, Flags::EVENTFLAG_IS_KEY_PAD),
            (flag::IS_LEFT, Flags::EVENTFLAG_IS_LEFT),
            (flag::IS_RIGHT, Flags::EVENTFLAG_IS_RIGHT),
            (flag::ALTGR_DOWN, Flags::EVENTFLAG_ALTGR_DOWN),
            (flag::IS_REPEAT, Flags::EVENTFLAG_IS_REPEAT),
        ] {
            assert_eq!(i64::from(ours), i64::from(cef.0), "{cef:?}");
        }
    }

    /// W's native code on the platform
    const W: i32 = if cfg!(target_os = "windows") {
        0x11
    } else if cfg!(target_os = "macos") {
        0x0d
    } else {
        0x19
    };

    fn press(virtual_key: i32, modifiers: u32, character: u8, unmodified: u8) -> KeyPress {
        let event = KeyEvent {
            windows_key_code: virtual_key,
            native_key_code: W,
            modifiers,
            character: u16::from(character),
            unmodified_character: u16::from(unmodified),
            ..Default::default()
        };
        KeyPress::from_cef(&event, None)
    }

    #[test]
    fn a_key_is_its_place_and_its_meaning() {
        let shift_w = press(0x57, flag::SHIFT_DOWN, b'W', b'W');
        assert_eq!(shift_w.code(), PhysicalCode::KeyW);
        assert_eq!(shift_w.key(), &LogicalKey::Character("W".into()));
        assert_eq!(shift_w.text(), Some("W"));

        // Ctrl turns w into a control character
        let ctrl_w = press(0x57, flag::CONTROL_DOWN, 0x17, b'w');
        assert_eq!(ctrl_w.key(), &LogicalKey::Character("w".into()));
        assert_eq!(ctrl_w.text(), None);

        for (virtual_key, character, key) in [
            (0x74, 0, LogicalKey::F5),
            (0x0d, b'\r', LogicalKey::Enter),
            (0x1b, 0x1b, LogicalKey::Escape),
            (0x2e, 0x7f, LogicalKey::Delete),
            (0xa1, 0, LogicalKey::Shift),
            (0xde, 0, LogicalKey::Unidentified),
        ] {
            let press = press(virtual_key, 0, character, character);
            assert_eq!(press.key(), &key, "{virtual_key:#x}");
            assert_eq!(press.text(), None, "{virtual_key:#x}");
        }
    }

    #[test]
    fn a_ctrl_chord_means_its_character_without_ctrl() {
        const CTRL: u32 = flag::CONTROL_DOWN;
        let character = |text: &str| LogicalKey::Character(text.into());
        for (virtual_key, modifiers, control, key) in [
            (0x54, CTRL, 0x14, character("t")),
            (0x54, CTRL | flag::SHIFT_DOWN, 0x14, character("T")),
            (0x54, CTRL | flag::CAPS_LOCK_ON, 0x14, character("T")),
            (0x31, CTRL, 0x11, character("1")),
            (0x31, CTRL | flag::SHIFT_DOWN, 0x01, character("!")),
            (0x20, CTRL, 0x00, character(" ")),
            (0xbd, CTRL, 0x0d, character("-")),
            // = shares its control character with ] and }
            (0xbb, CTRL, 0x1d, character("=")),
            // Another layout's # on the US layout's ' key
            (0xde, CTRL, 0x03, character("#")),
            (0xde, CTRL, 0x1d, LogicalKey::Unidentified),
            (0x0d, CTRL, 0x0a, LogicalKey::Enter),
            (0x54, 0, 0x14, LogicalKey::Unidentified),
        ] {
            let press = press(virtual_key, modifiers, control, control);
            assert_eq!(press.key(), &key, "{virtual_key:#x} {modifiers:#x}");
            assert_eq!(press.text(), None, "{virtual_key:#x} {modifiers:#x}");
        }
    }

    #[test]
    fn native_codes_name_the_place() {
        #[cfg(target_os = "windows")]
        let codes = [
            (0x11, PhysicalCode::KeyW),
            (0xe04d, PhysicalCode::ArrowRight),
            // lParams of a W press and a right arrow's repeat
            (0x0011_0001, PhysicalCode::KeyW),
            (0x414d_0001, PhysicalCode::ArrowRight),
            (0, PhysicalCode::Unidentified),
        ];
        #[cfg(target_os = "linux")]
        let codes = [
            (0x19, PhysicalCode::KeyW),
            (0x72, PhysicalCode::ArrowRight),
            (0, PhysicalCode::Unidentified),
        ];
        #[cfg(target_os = "macos")]
        let codes = [
            (0x0d, PhysicalCode::KeyW),
            (0x7c, PhysicalCode::ArrowRight),
            (0xffff, PhysicalCode::Unidentified),
        ];
        for (native, code) in codes {
            assert_eq!(physical_code(native), code, "{native:#x}");
        }
    }

    #[test]
    fn the_primary_modifier_is_the_platform_s() {
        let ctrl_w = press(0x57, flag::CONTROL_DOWN, 0x17, b'w');
        let cmd_w = press(0x57, flag::COMMAND_DOWN, b'w', b'w');
        assert_eq!(ctrl_w.modifiers().primary(), !cfg!(target_os = "macos"));
        assert_eq!(cmd_w.modifiers().primary(), cfg!(target_os = "macos"));

        let shift_w = press(0x57, flag::SHIFT_DOWN, b'W', b'W');
        assert!(shift_w.modifiers().shift() && !shift_w.modifiers().none());

        let held = press(0x28, flag::IS_REPEAT, 0, 0);
        assert!(held.repeat() && held.modifiers().none());
        assert_eq!(held.modifiers().chord(), 0);
        assert_eq!(held.command(), None);
        assert!(!held.in_editable_field());
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
