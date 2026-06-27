//! IPC message router
//!
//! Central dispatch layer for all IPC messages between browser and renderer.
//! Owns the per-subsystem handler maps and routes incoming messages by subsystem.

use cef::*;

use crate::browser_registry::BrowserId;
use crate::debug;
use crate::ipc::browser_state::IpcContext;
use crate::ipc::envelope::*;
use crate::ipc::binary::BinarySubsystem;
use crate::ipc::event::EventSubsystem;
use crate::ipc::rpc::RpcSubsystem;
use crate::ipc::stream::StreamSubsystem;

/// Top-level IPC router that owns all subsystems.
pub struct IpcRouter {
    pub rpc: RpcSubsystem,
    pub binary: BinarySubsystem,
    pub event: EventSubsystem,
    pub stream: StreamSubsystem,
}

/// Standalone renderer-side dispatcher.
///
/// Routes a decoded envelope + payload to the appropriate subsystem handler.
/// Does NOT require an IpcRouter instance, so it works in both browser and renderer processes.
pub fn route_renderer(frame: &mut Frame, envelope: &Envelope, payload: &[u8]) -> bool {
    match envelope.subsystem {
        SUB_RPC => crate::ipc::rpc::renderer::handle_rpc_renderer(frame, envelope, payload),
        SUB_BINARY => crate::ipc::binary::renderer::handle_binary_renderer(frame, envelope, payload),
        SUB_EVENT => crate::ipc::event::renderer::handle_event_renderer(frame, envelope, payload),
        SUB_STREAM => crate::ipc::stream::renderer::handle_stream_renderer(frame, envelope, payload),
        _ => {
            debug!("[Router Renderer] unknown subsystem {}", envelope.subsystem);
            false
        }
    }
}

impl IpcRouter {
    pub fn new(rpc: RpcSubsystem, binary: BinarySubsystem, event: EventSubsystem, stream: StreamSubsystem) -> Self {
        Self { rpc, binary, event, stream }
    }

    /// Route a message received from the renderer (browser-side dispatch).
    pub fn route_browser(
        &self,
        frame: &mut Frame,
        envelope: &Envelope,
        payload: &[u8],
        ctx: IpcContext,
    ) -> bool {
        match envelope.subsystem {
            SUB_RPC => {
                self.rpc.handle_browser(
                    frame,
                    envelope,
                    payload,
                    ctx,
                    self.rpc.pending.clone(),
                )
            }
            SUB_BINARY => {
                self.binary.handle_browser(
                    frame,
                    envelope,
                    payload,
                    ctx,
                    self.binary.pending.clone(),
                )
            }
            SUB_EVENT => {
                self.event.handle_browser(frame, envelope, payload, ctx)
            }
            SUB_STREAM => {
                self.stream.handle_browser(
                    frame,
                    envelope,
                    payload,
                    ctx,
                    self.stream.pending.clone(),
                )
            }
            _ => {
                debug!(
                    "[Router Browser] unknown subsystem {}",
                    envelope.subsystem
                );
                false
            }
        }
    }

    /// Route a message received from the browser (renderer-side dispatch).
    pub fn route_renderer(&self, frame: &mut Frame, envelope: &Envelope, payload: &[u8]) -> bool {
        route_renderer(frame, envelope, payload)
    }

    /// Cancel all pending async handlers for a given browser.
    pub fn cancel_all_for_browser(&self, browser_id: BrowserId) -> usize {
        let rpc_count = self.rpc.pending.cancel_all_for_browser(browser_id);
        let bin_count = self.binary.pending.cancel_all_for_browser(browser_id);
        let stream_count = self.stream.pending.cancel_all_for_browser(browser_id);
        // Clean up event subscriptions and stream state
        self.event.clear_for_browser(browser_id);
        self.stream.clear_for_browser(browser_id);
        let total = rpc_count + bin_count + stream_count;
        if total > 0 {
            debug!("[Router] canceled {} pending handlers for browser", total);
        }
        total
    }
}
