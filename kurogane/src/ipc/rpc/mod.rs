//! JSON-based request/response IPC subsystem.
//!
//! Supports synchronous and asynchronous command handlers, pending
//! request tracking, cancellation and promise resolution.

use crate::ipc::browser_state::{IpcContext, IpcResult};
use crate::ipc::responder::Responder;

pub type SyncRpcHandler = Box<dyn Fn(&str, IpcContext) -> IpcResult + Send + Sync>;
pub type AsyncRpcHandler = Box<dyn Fn(serde_json::Value, IpcResponder, IpcContext) + Send + Sync>;
pub type IpcResponder = Responder<String>;

pub mod renderer;
