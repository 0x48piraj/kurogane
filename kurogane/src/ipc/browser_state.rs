//! Browser-process IPC dispatch and transaction state.
//!
//! Defines the error type handlers return and the context the browser
//! process records for every message.

use std::fmt;
use std::num::NonZeroU16;

use cef::{CefStringUtf16, Frame, ImplFrame};

use crate::acl::Origin;
use crate::browser_registry::BrowserId;
use crate::ipc::envelope::FLAG_OPAQUE_CONTEXT;

/// Classifies an [`IpcError`] by the numeric code exposed to the renderer.
/// Runtime errors use the fixed codes `0` through `-8`; applications may use
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
    /// The file exceeds the transfer limit (`-8`).
    TooLarge,
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

/// Identifies the frame a message came from (CEF's frame identifier).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FrameId(String);

impl FrameId {
    #[cfg(test)]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// The identifier of a CEF frame.
    pub fn of(frame: &Frame) -> Self {
        let id: CefStringUtf16 = (&frame.identifier()).into();
        Self(id.to_string())
    }
}

/// Context for an IPC dispatch.
pub struct IpcContext {
    pub browser_id: Option<BrowserId>,
    /// Frame that sent the message.
    pub frame: FrameId,
    /// Origin the message acts for.
    pub origin: Origin,
    /// URL origin of the document when the message arrived.
    pub url_origin: Origin,
}

/// Returns the origin a message acts for.
///
/// An opaque renderer context acts for the opaque origin ([`FLAG_OPAQUE_CONTEXT`]).
pub(crate) fn effective_origin(url_origin: &Origin, flags: u8) -> Origin {
    if flags & FLAG_OPAQUE_CONTEXT != 0 {
        Origin::OPAQUE
    } else {
        url_origin.clone()
    }
}

/// Returns whether `current_url` still has the origin addressed by the message.
///
/// A navigation to another origin makes the message no longer addressed.
pub(crate) fn still_addressed(url_origin: &Origin, current_url: &str) -> bool {
    Origin::from_url(current_url) == *url_origin
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_opaque_context_acts_for_the_opaque_origin() {
        let app = Origin::parse("app://app").unwrap();
        assert_eq!(effective_origin(&app, 0), app);
        assert_eq!(effective_origin(&app, FLAG_OPAQUE_CONTEXT), Origin::OPAQUE);
        assert_eq!(
            effective_origin(&Origin::OPAQUE, 0),
            Origin::OPAQUE,
            "never raised"
        );
    }

    #[test]
    fn answers_follow_only_the_same_document_origin() {
        let app = Origin::parse("app://app").unwrap();
        assert!(still_addressed(&app, "app://app/page#x"));
        assert!(!still_addressed(&app, "https://evil.example/"));
        assert!(!still_addressed(&app, "about:blank"));
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
