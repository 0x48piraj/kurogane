//! Browser-process IPC dispatch and transaction state.
//!
//! Defines the immutable command dispatcher used by the browser process
//! and the runtime state required for active IPC transactions.

use crate::browser_registry::BrowserId;

pub type IpcResult = Result<String, String>;

/// Contextual information for an IPC dispatch call.
pub struct IpcContext {
    pub browser_id: Option<BrowserId>,
    pub frame_id: Option<i64>,
}
