//! Binary IPC subsystem.
//!
//! Request/response IPC for arbitrary binary payloads.
//! Supports synchronous and asynchronous handlers, cancellation of
//! pending requests and dispatch of binary responses or structured error payloads.

use crate::ipc::browser_state::IpcContext;
use crate::ipc::responder::Responder;

pub type SyncBinaryHandler = Box<dyn Fn(&[u8], IpcContext) -> Result<Vec<u8>, String> + Send + Sync>;
pub type AsyncBinaryHandler = Box<dyn Fn(&[u8], BinaryResponder, IpcContext) + Send + Sync>;
pub type BinaryResponder = Responder<Vec<u8>>;

pub mod renderer;
