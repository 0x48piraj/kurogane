//! Browser-process IPC dispatch and transaction state.
//!
//! Defines the immutable command dispatcher used by the browser process
//! and the runtime state required for active IPC transactions.

use crate::browser_registry::BrowserId;

pub type IpcResult = Result<String, String>;

/// A structured error with a numeric code and human-readable message.
///
/// Displays as "{code}: {message}", used as the serialized error format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IpcError {
    pub message: String,
    pub code: i32,
}

impl IpcError {
    /// Code used for handler-reported errors (default)
    pub const CODE_HANDLER: i32 = 0;
    /// Code used when a handler panics
    pub const CODE_PANIC: i32 = -1;
    /// Code used for buffer/serialization failures
    pub const CODE_BUFFER: i32 = -2;
    /// Code used when a responder is dropped without resolving
    pub const CODE_DROPPED: i32 = -3;

    pub fn new(message: impl Into<String>, code: i32) -> Self {
        Self {
            message: message.into(),
            code,
        }
    }

    pub fn code(&self) -> i32 {
        self.code
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl std::fmt::Display for IpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for IpcError {}

impl From<String> for IpcError {
    fn from(message: String) -> Self {
        Self::new(message, Self::CODE_HANDLER)
    }
}

impl From<&str> for IpcError {
    fn from(message: &str) -> Self {
        Self::new(message.to_string(), Self::CODE_HANDLER)
    }
}

impl From<serde_json::Error> for IpcError {
    fn from(e: serde_json::Error) -> Self {
        Self::new(e.to_string(), Self::CODE_BUFFER)
    }
}

/// Contextual information for an IPC dispatch call.
pub struct IpcContext {
    pub browser_id: Option<BrowserId>,
    pub frame_id: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ipc_error_display_format() {
        let err = IpcError::new("handler panicked", -1);
        assert_eq!(format!("{err}"), "-1: handler panicked");
    }

    #[test]
    fn ipc_error_display_zero_code() {
        let err = IpcError::new("invalid JSON: unexpected token", 0);
        assert_eq!(format!("{err}"), "0: invalid JSON: unexpected token");
    }

    #[test]
    fn ipc_error_display_negative_code() {
        let err = IpcError::new("handler dropped responder without resolving", -3);
        assert_eq!(
            format!("{err}"),
            "-3: handler dropped responder without resolving"
        );
    }

    #[test]
    fn ipc_error_display_large_positive_code() {
        let err = IpcError::new("custom error", i32::MAX);
        assert_eq!(format!("{err}"), format!("{}: custom error", i32::MAX));
    }

    #[test]
    fn ipc_error_display_empty_message() {
        let err = IpcError::new("", -1);
        assert_eq!(format!("{err}"), "-1: ");
    }

    #[test]
    fn ipc_error_new_stores_fields() {
        let err = IpcError::new("test message", 42);
        assert_eq!(err.message, "test message");
        assert_eq!(err.code, 42);
    }

    #[test]
    fn ipc_error_clone() {
        let err = IpcError::new("clone me", -7);
        let cloned = err.clone();
        assert_eq!(cloned.message, "clone me");
        assert_eq!(cloned.code, -7);
    }

    #[test]
    fn ipc_error_debug_format() {
        let err = IpcError::new("debug test", 5);
        let debug = format!("{:?}", err);
        assert!(debug.contains("debug test"));
        assert!(debug.contains("5"));
    }

    #[test]
    fn ipc_error_code_constants() {
        assert_eq!(IpcError::CODE_HANDLER, 0);
        assert_eq!(IpcError::CODE_PANIC, -1);
        assert_eq!(IpcError::CODE_BUFFER, -2);
        assert_eq!(IpcError::CODE_DROPPED, -3);
    }

    #[test]
    fn ipc_error_from_string() {
        let err: IpcError = "boom".to_string().into();
        assert_eq!(err.code(), IpcError::CODE_HANDLER);
        assert_eq!(err.message(), "boom");
    }

    #[test]
    fn ipc_error_from_str() {
        let err: IpcError = "boom".into();
        assert_eq!(err.code(), IpcError::CODE_HANDLER);
        assert_eq!(err.message(), "boom");
    }

    #[test]
    fn ipc_error_from_serde_json() {
        let inner = serde_json::from_str::<i32>("nope").unwrap_err();
        let err: IpcError = inner.into();
        assert_eq!(err.code(), IpcError::CODE_BUFFER);
        assert!(!err.message().is_empty());
    }

    #[test]
    fn ipc_error_is_std_error() {
        let err = IpcError::new("boom", -7);
        let source = std::error::Error::source(&err);
        assert!(source.is_none());
    }
}
