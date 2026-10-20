//! Browser-process IPC errors and message context.
//!
//! Defines the error type handlers return and the context the browser
//! process records for every message.

use std::fmt;
use std::num::NonZeroU16;
use std::sync::Arc;

use tanso::{CefStringUtf16, Frame, ImplFrame};

use crate::acl::Origin;
use crate::ipc::document::Document;
use crate::ipc::envelope::FLAG_OPAQUE_CONTEXT;

/// Classifies an [`IpcError`] by the numeric code exposed to the renderer.
/// Runtime errors use the fixed codes `0` through `-9`; applications may use
/// positive codes through [`ErrorCode::App`].
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ErrorCode {
    /// A handler reported a failure (`0`, the default).
    Handler,
    /// The handler panicked (`-1`).
    Panic,
    /// The request or response could not be decoded or encoded (`-2`).
    Buffer,
    /// The handler dropped its responder without resolving (`-3`).
    Dropped,
    /// The ACL refused the calling origin (`-4`).
    Acl,
    /// No capability grant of the origin covers the operation (`-5`).
    Capability,
    /// The path is outside the granted roots, denied, or a link (`-6`).
    PathDenied,
    /// The path is malformed (`-7`).
    PathInvalid,
    /// The file or message exceeds its transfer limit (`-8`).
    TooLarge,
    /// The page left with the request open and came back from the
    /// back-forward cache (`-9`).
    Left,
    /// An application-defined code, sent as is.
    App(NonZeroU16),
}

impl ErrorCode {
    /// The numeric code the renderer receives.
    pub fn wire(self) -> i32 {
        match self {
            ErrorCode::Handler => 0,
            ErrorCode::Panic => -1,
            ErrorCode::Buffer => -2,
            ErrorCode::Dropped => -3,
            ErrorCode::Acl => -4,
            ErrorCode::Capability => -5,
            ErrorCode::PathDenied => -6,
            ErrorCode::PathInvalid => -7,
            ErrorCode::TooLarge => -8,
            ErrorCode::Left => -9,
            ErrorCode::App(code) => i32::from(code.get()),
        }
    }
}

/// A structured error with a numeric [`ErrorCode`] and human-readable message.
///
/// Displays as `"{code}: {message}"`, the format the renderer parses into an
/// `Error` with a numeric `.code`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IpcError {
    message: String,
    code: ErrorCode,
}

impl IpcError {
    /// A handler failure ([`ErrorCode::Handler`]).
    pub fn new(message: impl Into<String>) -> Self {
        Self::with_code(message, ErrorCode::Handler)
    }

    /// A failure of class `code`.
    pub fn with_code(message: impl Into<String>, code: ErrorCode) -> Self {
        Self {
            message: message.into(),
            code,
        }
    }

    pub fn code(&self) -> ErrorCode {
        self.code
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for IpcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code.wire(), self.message)
    }
}

impl std::error::Error for IpcError {}

impl From<String> for IpcError {
    fn from(message: String) -> Self {
        Self::new(message)
    }
}

impl From<&str> for IpcError {
    fn from(message: &str) -> Self {
        Self::new(message)
    }
}

impl From<serde_json::Error> for IpcError {
    fn from(e: serde_json::Error) -> Self {
        Self::with_code(e.to_string(), ErrorCode::Buffer)
    }
}

/// CEF's identifier of a frame. Clones share one string.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FrameId(Arc<str>);

impl FrameId {
    #[cfg(test)]
    pub fn new(id: &str) -> Self {
        Self(Arc::from(id))
    }

    /// Returns the identifier of a CEF frame.
    pub fn of(frame: &Frame) -> Self {
        let id: CefStringUtf16 = (&frame.identifier()).into();
        Self(shared(id.as_slice().unwrap_or_default()))
    }
}

/// Returns UTF-16 `units` as a shared string made with one allocation.
fn shared(units: &[u16]) -> Arc<str> {
    // Frame identifiers are short; a longer string takes the general path
    let mut utf8 = [0u8; 64];
    let mut len = 0;
    for unit in char::decode_utf16(units.iter().copied()) {
        let c = unit.unwrap_or(char::REPLACEMENT_CHARACTER);
        let Some(slot) = utf8.get_mut(len..len + c.len_utf8()) else {
            return Arc::from(String::from_utf16_lossy(units));
        };
        len += c.encode_utf8(slot).len();
    }
    // Whole characters, each encoded above
    Arc::from(std::str::from_utf8(&utf8[..len]).unwrap_or_default())
}

/// Origin of a message from an opaque context.
static OPAQUE: Origin = Origin::OPAQUE;

/// Context for an IPC dispatch.
pub struct IpcContext {
    /// Document that sent the message.
    pub document: Arc<Document>,
    /// Whether the message comes from a context with an opaque origin
    /// ([`FLAG_OPAQUE_CONTEXT`]).
    opaque: bool,
}

impl IpcContext {
    /// A message of `document` carrying envelope `flags`.
    pub(crate) fn new(document: Arc<Document>, flags: u8) -> Self {
        Self {
            document,
            opaque: flags & FLAG_OPAQUE_CONTEXT != 0,
        }
    }

    /// Returns the origin the message acts for. A message from an opaque
    /// context acts for the opaque origin, never its document's.
    pub fn origin(&self) -> &Origin {
        if self.opaque {
            &OPAQUE
        } else {
            self.document.origin()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_opaque_context_acts_for_the_opaque_origin() {
        let app = Origin::parse("app://app").unwrap();
        let document = Document::for_test(app.clone());
        assert_eq!(IpcContext::new(document.clone(), 0).origin(), &app);
        assert_eq!(
            IpcContext::new(document, FLAG_OPAQUE_CONTEXT).origin(),
            &Origin::OPAQUE
        );
        assert_eq!(
            IpcContext::new(Document::for_test(Origin::OPAQUE), 0).origin(),
            &Origin::OPAQUE,
            "never raised"
        );
    }

    #[test]
    fn a_frame_identifier_reads_whole_at_any_length() {
        let utf16 = |text: &str| text.encode_utf16().collect::<Vec<_>>();
        let short = "5-B8FE64B798E8802F8727D5F90A7428E0";
        assert_eq!(&*shared(&utf16(short)), short);
        let long = "x".repeat(200);
        assert_eq!(&*shared(&utf16(&long)), long);
        assert_eq!(&*shared(&utf16("frame-ü-😀")), "frame-ü-😀");
        assert_eq!(&*shared(&[0xD800]), "\u{FFFD}", "a lone surrogate");
        assert_eq!(&*shared(&[]), "");
    }

    #[test]
    fn displays_the_wire_code_and_the_message() {
        assert_eq!(IpcError::new("boom").to_string(), "0: boom");
        assert_eq!(
            IpcError::with_code("handler panicked", ErrorCode::Panic).to_string(),
            "-1: handler panicked"
        );
        let custom = ErrorCode::App(NonZeroU16::new(42).unwrap());
        assert_eq!(
            IpcError::with_code("custom", custom).to_string(),
            "42: custom"
        );
        assert_eq!(IpcError::new("").to_string(), "0: ");
    }

    #[test]
    fn runtime_classes_keep_their_wire_codes() {
        let classes = [
            (ErrorCode::Handler, 0),
            (ErrorCode::Panic, -1),
            (ErrorCode::Buffer, -2),
            (ErrorCode::Dropped, -3),
            (ErrorCode::Acl, -4),
            (ErrorCode::Capability, -5),
            (ErrorCode::PathDenied, -6),
            (ErrorCode::PathInvalid, -7),
            (ErrorCode::TooLarge, -8),
        ];
        for (code, wire) in classes {
            assert_eq!(code.wire(), wire, "{code:?}");
        }
        assert_eq!(ErrorCode::App(NonZeroU16::MAX).wire(), 65535);
    }

    #[test]
    fn conversions_pick_the_right_class() {
        let from_string: IpcError = "boom".to_string().into();
        let from_str: IpcError = "boom".into();
        assert_eq!(from_string.code(), ErrorCode::Handler);
        assert_eq!(from_str.message(), "boom");
        let json: IpcError = serde_json::from_str::<i32>("nope").unwrap_err().into();
        assert_eq!(json.code(), ErrorCode::Buffer);
        assert!(!json.message().is_empty());
    }

    #[test]
    fn is_a_std_error_without_a_source() {
        let err = IpcError::new("boom");
        assert!(std::error::Error::source(&err).is_none());
    }
}
