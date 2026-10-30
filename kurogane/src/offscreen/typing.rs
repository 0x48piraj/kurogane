//! Key and IME input to an offscreen browser.
//!
//! A key goes to CEF as each OS's own events make it. Windows and Linux read
//! the virtual key and the native code, macOS the characters and the native
//! code. Text of one UTF-16 unit follows the key-down as a character event;
//! longer text is committed whole.

use std::os::raw::c_int;

use tanso::{CompositionUnderline, CompositionUnderlineStyle, KeyEvent, KeyEventType, Range};

use crate::keyboard::{LogicalKey, PhysicalCode};
use crate::keys::flag;

/// A key for [`OffscreenBrowser::key_down`](crate::OffscreenBrowser::key_down)
/// and [`key_up`](crate::OffscreenBrowser::key_up), by its place and its
/// meaning in the layout.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyInput {
    code: PhysicalCode,
    key: LogicalKey,
    text: Option<String>,
    repeat: bool,
}

impl KeyInput {
    /// The key at `code` meaning `key` in the layout, typing nothing.
    pub fn new(code: PhysicalCode, key: LogicalKey) -> Self {
        Self {
            code,
            key,
            text: None,
            repeat: false,
        }
    }

    /// Returns the key typing `text` when pressed, as the OS reports it with
    /// the key. An empty text types nothing.
    pub fn text(self, text: impl Into<String>) -> Self {
        let text = text.into();
        Self {
            text: (!text.is_empty()).then_some(text),
            ..self
        }
    }

    /// Returns the key as a repeat of one held down.
    pub fn repeat(self, repeat: bool) -> Self {
        Self { repeat, ..self }
    }

    /// Returns CEF's events of pressing the key with the held `modifiers`.
    pub(crate) fn press(&self, modifiers: u32) -> Press {
        let units: Vec<u16> = self
            .text
            .as_deref()
            .map_or_else(Vec::new, |text| text.encode_utf16().collect());
        let down = self.event(KeyEventType::RAWKEYDOWN, modifiers);
        match units.as_slice() {
            [] => Press {
                down,
                character: None,
                commit: None,
            },
            &[unit] => Press {
                character: Some(self.character_event(unit, modifiers)),
                down,
                commit: None,
            },
            _ => Press {
                down,
                character: None,
                commit: self.text.clone(),
            },
        }
    }

    /// Returns CEF's event of releasing the key with the held `modifiers`.
    pub(crate) fn release(&self, modifiers: u32) -> KeyEvent {
        self.event(KeyEventType::KEYUP, modifiers)
    }

    /// Returns the key's event of `kind`.
    fn event(&self, kind: KeyEventType, modifiers: u32) -> KeyEvent {
        let virtual_key = self.virtual_key();
        let flags = modifiers | self.location() | if self.repeat { flag::IS_REPEAT } else { 0 };
        let text_unit = self.text.as_deref().and_then(single_unit);
        let key_unit = match &self.key {
            LogicalKey::Character(text) => single_unit(text),
            _ => None,
        };
        // macOS builds an NSEvent of the characters; a named key types its
        // function key's character, and one of no characters is a modifier
        #[cfg(target_os = "macos")]
        let key_unit = key_unit.or_else(|| crate::keyboard::mac_character(virtual_key));
        let character = text_unit.or(key_unit).unwrap_or(0);
        KeyEvent {
            type_: kind,
            modifiers: flags,
            windows_key_code: virtual_key as c_int,
            native_key_code: self.native_code(),
            is_system_key: system_key(flags).into(),
            character,
            unmodified_character: key_unit.unwrap_or(character),
            ..Default::default()
        }
    }

    /// Returns the character event of the text's one UTF-16 unit `unit`.
    /// Windows takes it in the virtual key's place; text typed with Ctrl and
    /// Alt both held is AltGr's.
    fn character_event(&self, unit: u16, modifiers: u32) -> KeyEvent {
        let mut event = self.event(KeyEventType::CHAR, modifiers);
        event.character = unit;
        if cfg!(target_os = "windows") {
            event.windows_key_code = c_int::from(unit);
        }
        let ctrl_alt = flag::CONTROL_DOWN | flag::ALT_DOWN;
        if event.modifiers & ctrl_alt == ctrl_alt {
            event.modifiers = event.modifiers & !ctrl_alt | flag::ALTGR_DOWN;
            event.is_system_key = 0;
        }
        event
    }

    /// Returns the key's Windows virtual key: a letter's or digit's own, a
    /// named key's, else its place's in the US layout.
    fn virtual_key(&self) -> u32 {
        let from_key = match &self.key {
            LogicalKey::Character(text) => match text.as_bytes() {
                &[c] if c.is_ascii_alphanumeric() => Some(u32::from(c.to_ascii_uppercase())),
                _ => None,
            },
            named => named.virtual_key(),
        };
        from_key.or_else(|| self.code.us_virtual_key()).unwrap_or(0)
    }

    /// Returns CEF's flags of where the key sits: a side for a modifier, the
    /// keypad for a keypad key.
    fn location(&self) -> u32 {
        match self.code {
            PhysicalCode::ShiftLeft
            | PhysicalCode::ControlLeft
            | PhysicalCode::AltLeft
            | PhysicalCode::MetaLeft => flag::IS_LEFT,
            PhysicalCode::ShiftRight
            | PhysicalCode::ControlRight
            | PhysicalCode::AltRight
            | PhysicalCode::MetaRight => flag::IS_RIGHT,
            code if code.as_str().starts_with("Numpad") => flag::IS_KEY_PAD,
            _ => 0,
        }
    }

    /// Returns the OS's native code of the key's place.
    fn native_code(&self) -> c_int {
        #[cfg(target_os = "windows")]
        let code = self.code.windows_scan_code().unwrap_or(0);
        #[cfg(target_os = "linux")]
        let code = self.code.xkb_keycode().unwrap_or(0);
        // A Mac key without a code takes none of the Mac's, 0 being A's
        #[cfg(target_os = "macos")]
        let code = self.code.mac_keycode().unwrap_or(u16::MAX);
        c_int::from(code)
    }
}

/// CEF's events of a key press.
pub(crate) struct Press {
    pub(crate) down: KeyEvent,
    /// Character event of text of one UTF-16 unit.
    pub(crate) character: Option<KeyEvent>,
    /// Longer text, committed whole.
    pub(crate) commit: Option<String>,
}

/// Returns whether a key with `flags` is a system key: Cmd on macOS, Alt
/// without Ctrl elsewhere.
fn system_key(flags: u32) -> bool {
    if cfg!(target_os = "macos") {
        flags & flag::COMMAND_DOWN != 0
    } else {
        flags & flag::ALT_DOWN != 0 && flags & flag::CONTROL_DOWN == 0
    }
}

/// Returns the one UTF-16 unit `text` is made of, None for more or none.
fn single_unit(text: &str) -> Option<u16> {
    let mut units = text.encode_utf16();
    match (units.next(), units.next()) {
        (Some(unit), None) => Some(unit),
        _ => None,
    }
}

/// CEF's range for no range.
pub(crate) const NO_RANGE: Range = Range {
    from: u32::MAX,
    to: u32::MAX,
};

/// Returns the UTF-16 offset of byte offset `byte` into `text`, from the
/// character boundary at or before it.
fn utf16_offset(text: &str, byte: usize) -> u32 {
    let byte = text.floor_char_boundary(byte);
    u32::try_from(text[..byte].encode_utf16().count()).unwrap_or(u32::MAX)
}

/// Returns CEF's selection of a composition of `text` with the cursor as
/// byte offsets `cursor`, at its end for none.
pub(crate) fn preedit_selection(text: &str, cursor: Option<(usize, usize)>) -> Range {
    let (start, end) = cursor.unwrap_or((text.len(), text.len()));
    Range {
        from: utf16_offset(text, start),
        to: utf16_offset(text, end),
    }
}

/// Returns the underline of a composition of `text`: thin, black and solid
/// across it, as an IME without styles of its own shows one.
pub(crate) fn preedit_underline(text: &str) -> CompositionUnderline {
    CompositionUnderline {
        range: Range {
            from: 0,
            to: utf16_offset(text, text.len()),
        },
        color: 0xFF00_0000,
        background_color: 0,
        thick: 0,
        style: CompositionUnderlineStyle::SOLID,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(code: PhysicalCode, key: &str) -> KeyInput {
        KeyInput::new(code, LogicalKey::from_w3c(key))
    }

    #[test]
    fn letters_digits_and_named_keys_have_their_virtual_keys() {
        assert_eq!(input(PhysicalCode::KeyA, "a").virtual_key(), 0x41);
        assert_eq!(input(PhysicalCode::KeyQ, "A").virtual_key(), 0x41);
        assert_eq!(input(PhysicalCode::Digit1, "1").virtual_key(), 0x31);
        assert_eq!(input(PhysicalCode::Enter, "Enter").virtual_key(), 0x0d);
        assert_eq!(
            input(PhysicalCode::ArrowLeft, "ArrowLeft").virtual_key(),
            0x25
        );
        // Any other character takes its place's US virtual key
        assert_eq!(input(PhysicalCode::Digit2, "é").virtual_key(), 0x32);
        assert_eq!(input(PhysicalCode::Equal, "=").virtual_key(), 0xbb);
    }

    #[test]
    fn text_of_one_unit_is_a_character_event_and_longer_text_a_commit() {
        let a = input(PhysicalCode::KeyA, "a").text("a").press(0);
        assert_eq!(a.down.type_, KeyEventType::RAWKEYDOWN);
        assert_eq!(
            a.character.map(|event| event.character),
            Some(u16::from(b'a'))
        );
        assert_eq!(a.commit, None);

        let emoji = input(PhysicalCode::KeyA, "😀").text("😀").press(0);
        assert!(emoji.character.is_none());
        assert_eq!(emoji.commit.as_deref(), Some("😀"));

        let arrow = input(PhysicalCode::ArrowLeft, "ArrowLeft")
            .text("")
            .press(0);
        assert!(arrow.character.is_none() && arrow.commit.is_none());
    }

    #[test]
    fn flags_tell_side_keypad_repeat_and_altgr() {
        let shift = input(PhysicalCode::ShiftRight, "Shift").press(flag::SHIFT_DOWN);
        assert_eq!(shift.down.modifiers, flag::SHIFT_DOWN | flag::IS_RIGHT);
        let pad = input(PhysicalCode::Numpad1, "1").repeat(true).release(0);
        assert_eq!(pad.modifiers, flag::IS_KEY_PAD | flag::IS_REPEAT);
        let altgr = input(PhysicalCode::KeyE, "€")
            .text("€")
            .press(flag::CONTROL_DOWN | flag::ALT_DOWN);
        assert_eq!(
            altgr.character.map(|event| event.modifiers),
            Some(flag::ALTGR_DOWN)
        );
    }

    #[test]
    fn preedit_ranges_count_utf16_units() {
        // "aé😀" is 1 + 2 + 4 bytes and 1 + 1 + 2 units
        let text = "aé😀";
        let range = |from, to| Range { from, to };
        assert_eq!(preedit_selection(text, None), range(4, 4));
        assert_eq!(preedit_selection(text, Some((1, 3))), range(1, 2));
        // Inside a character counts from its start
        assert_eq!(preedit_selection(text, Some((2, 5))), range(1, 2));
        assert_eq!(preedit_underline(text).range, range(0, 4));
    }
}
