pub mod envelope;
pub mod transport {
    pub mod message;
}
pub mod browser_state;
pub mod renderer_state;
pub mod pending;
pub mod rpc;
pub mod binary_buffer;
pub mod utils;
pub mod responder;
pub mod event;
pub mod stream;
pub mod request_response;
pub mod router;
pub mod browser;
pub mod renderer;
pub mod handle_cell;

// Public exports for the rest of the application
pub use browser::handle_ipc_message;
pub use renderer::IpcRenderProcessHandler;
pub use browser_state::{IpcResult, IpcError, IpcContext};
pub use router::IpcRouter;
pub use request_response::{RequestResponseSubsystem, SyncHandler, AsyncHandler, BinaryResponder};
pub use responder::Responder;
pub use event::EventSubsystem;
pub use stream::{StreamSubsystem, StreamHandler, StreamFactory, StreamResponder};
pub use handle_cell::AppCell;
