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
    pub frame_id: Option<i64>,
}
