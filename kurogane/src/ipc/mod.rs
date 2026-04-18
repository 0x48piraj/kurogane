pub mod protocol;
pub mod transport {
    pub mod shm;
}
pub mod browser_state;
pub mod renderer_state;
pub mod rpc;
pub mod binary;
pub mod router;
pub mod browser;
pub mod renderer;

// Public exports for the rest of the application
pub use browser::handle_ipc_message;
pub use renderer::IpcRenderProcessHandler;
pub use browser_state::{
    init_dispatcher, 
    get_dispatcher, 
    register_command, 
    register_binary_command,
    IpcResult
};
