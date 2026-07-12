//! Browser-process IPC dispatch and transaction state.
//!
//! Defines the immutable command dispatcher used by the browser process
//! and the runtime state required for active IPC transactions.

use crate::browser_registry::BrowserId;

pub type IpcResult = Result<String, String>;

/// A structured error with a numeric code and human-readable message.
#[derive(Debug, Clone)]
pub struct IpcError {
    pub message: String,
    pub code: i32,
}

impl IpcError {
    pub fn new(message: impl Into<String>, code: i32) -> Self {
        Self { message: message.into(), code }
    }
}

impl std::fmt::Display for IpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
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
        assert_eq!(format!("{err}"), "-3: handler dropped responder without resolving");
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
}
