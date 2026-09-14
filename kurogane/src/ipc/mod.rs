pub(crate) mod envelope;
pub(crate) mod transport {
    pub(crate) mod message;
}
pub(crate) mod browser_state;
pub(crate) mod renderer_state;
pub(crate) mod pending;
pub(crate) mod rpc;
pub(crate) mod binary_buffer;
pub(crate) mod utils;
pub(crate) mod responder;
pub(crate) mod event;
pub(crate) mod stream;
pub(crate) mod request_response;
pub(crate) mod router;
pub(crate) mod browser;
pub(crate) mod renderer;
pub(crate) mod handle_cell;

// Public API: the types handlers see.
pub use browser_state::{ErrorCode, IpcError};
pub use request_response::BinaryResponder;
pub use responder::Responder;
pub use stream::{StreamHandler, StreamResponder};

// Runtime wiring, crate-internal.
pub(crate) use browser::handle_ipc_message;
pub(crate) use renderer::IpcRenderProcessHandler;
pub(crate) use browser_state::{FrameId, IpcContext};
pub(crate) use router::IpcRouter;
pub(crate) use request_response::{RequestResponseSubsystem, SyncHandler, AsyncHandler};
pub(crate) use event::EventSubsystem;
pub(crate) use stream::{StreamSubsystem, StreamFactory};
pub(crate) use handle_cell::AppCell;
