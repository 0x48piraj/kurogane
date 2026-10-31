//! Editing keys of offscreen browsers on macOS.
//!
//! Blink binds the editing keys itself on Windows and Linux. On macOS they
//! reach it through a view's responder chain, which an offscreen browser has
//! no part in. Kurogane runs them once the page has let the key through.

use tanso::*;

use crate::keyboard::LogicalKey;
use crate::keys::KeyPress;

/// Runs the editing command `press` names in the focused frame of `browser`.
/// Returns whether it names one.
pub(crate) fn run(browser: &Browser, press: &KeyPress) -> bool {
    let modifiers = press.modifiers();
    if !modifiers.meta() || modifiers.ctrl() || modifiers.alt() {
        return false;
    }
    let LogicalKey::Character(key) = press.key() else {
        return false;
    };
    let Some(frame) = browser.focused_frame() else {
        return false;
    };
    match (key.to_ascii_lowercase().as_str(), modifiers.shift()) {
        ("a", false) => frame.select_all(),
        ("c", false) => frame.copy(),
        ("x", false) => frame.cut(),
        ("v", false) => frame.paste(),
        ("z", false) => frame.undo(),
        ("z", true) => frame.redo(),
        _ => return false,
    }
    true
}
