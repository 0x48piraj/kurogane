pub mod envelope;
pub mod transport {
    pub mod message;
}
pub mod browser_state;
pub mod renderer_state;
pub mod rpc;
pub mod binary_buffer;
pub mod binary;
pub mod event;
pub mod stream;
pub mod router;
pub mod browser;
pub mod renderer;

// Public exports for the rest of the application
pub use browser::handle_ipc_message;
pub use renderer::IpcRenderProcessHandler;
pub use browser_state::{IpcResult, IpcContext};
pub use router::IpcRouter;
pub use rpc::{RpcSubsystem, IpcResponder, SyncRpcHandler, AsyncRpcHandler};
pub use binary::{BinarySubsystem, BinaryResponder, SyncBinaryHandler, AsyncBinaryHandler};
pub use event::EventSubsystem;
pub use stream::{StreamSubsystem, StreamHandler};
