//! Focused-field reports from the renderer.
//!
//! The renderer tells the browser what the focused field takes and, in the
//! main frame, where it sits. An offscreen browser hears them as its
//! [`InputPurpose`] and [`ImeArea`](super::OffscreenEvent::ImeArea).

use tanso::*;

use super::OffscreenClient;
use super::events::{InputPurpose, OffscreenEvent};
use super::frame::PixelRect;

/// Process message of a focused field from the renderer: the purpose's
/// number, then the field's bounds in device pixels for a field of the main
/// frame.
pub(crate) const FOCUSED_FIELD: &str = "kurogane-focused-field";

/// Purposes by their number in the message.
const PURPOSES: [InputPurpose; 8] = [
    InputPurpose::None,
    InputPurpose::Text,
    InputPurpose::Password,
    InputPurpose::Email,
    InputPurpose::Number,
    InputPurpose::Url,
    InputPurpose::Tel,
    InputPurpose::Search,
];

/// Returns what `node` takes: a text field's purpose by its `type`, Text
/// for a text area or editable element, None for any other node.
fn purpose_of(node: Option<&Domnode>) -> InputPurpose {
    let Some(node) = node.filter(|node| node.is_editable() != 0) else {
        return InputPurpose::None;
    };
    let tag = CefString::from(&node.element_tag_name()).to_string();
    if !tag.eq_ignore_ascii_case("input") {
        return InputPurpose::Text;
    }
    let kind = CefString::from(&node.element_attribute(&CefString::from("type"))).to_string();
    match kind.to_ascii_lowercase().as_str() {
        "password" => InputPurpose::Password,
        "email" => InputPurpose::Email,
        "number" => InputPurpose::Number,
        "url" => InputPurpose::Url,
        "tel" => InputPurpose::Tel,
        "search" => InputPurpose::Search,
        _ => InputPurpose::Text,
    }
}

/// Returns the message reporting `node` as the focused node, with its bounds
/// when it is a field of the main frame (`in_main_frame`). Renderer side.
pub(crate) fn report(in_main_frame: bool, node: Option<&Domnode>) -> Option<ProcessMessage> {
    let purpose = purpose_of(node);
    let message = process_message_create(&CefString::from(FOCUSED_FIELD))?;
    let args = message.argument_list()?;
    let number = PURPOSES
        .iter()
        .position(|&known| known == purpose)
        .unwrap_or(0);
    args.set_int(0, number as i32);
    if let Some(node) = node.filter(|_| in_main_frame && purpose != InputPurpose::None) {
        let bounds = node.element_bounds();
        for (index, value) in [bounds.x, bounds.y, bounds.width, bounds.height]
            .into_iter()
            .enumerate()
        {
            args.set_int(index + 1, value);
        }
    }
    Some(message)
}

/// Queues the purpose and area a focused-field `message` reports for
/// `offscreen`. Returns whether `message` was one. Browser side; the
/// renderer's numbers are checked, not trusted.
pub(crate) fn take(offscreen: &OffscreenClient, message: &ProcessMessage) -> bool {
    if !is_report(message) {
        return false;
    }
    let Some(args) = message.argument_list() else {
        return true;
    };
    let number = usize::try_from(args.int(0)).unwrap_or(0);
    let purpose = PURPOSES.get(number).copied().unwrap_or(InputPurpose::None);
    offscreen.push(OffscreenEvent::InputPurpose(purpose));
    if args.size() == 5 && purpose != InputPurpose::None {
        let rect = Rect {
            x: args.int(1),
            y: args.int(2),
            width: args.int(3),
            height: args.int(4),
        };
        offscreen.push(OffscreenEvent::ImeArea(PixelRect::from_cef(&rect)));
    }
    true
}

/// Returns whether `message` is a focused-field report.
pub(crate) fn is_report(message: &ProcessMessage) -> bool {
    CefString::from(&message.name()).to_string() == FOCUSED_FIELD
}
