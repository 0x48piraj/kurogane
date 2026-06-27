use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Arc;

use cef::*;

use crate::debug;
use crate::ipc::browser_state::{IpcContext, IpcResult};
use crate::ipc::envelope::*;
use crate::ipc::transport::message::build_message;
use crate::ipc::pending::{PendingEntry, PendingMap};
use crate::ipc::rpc::{IpcResponder, RpcSubsystem};

impl RpcSubsystem {
    /// Handle an RPC message arriving from the renderer (browser-side dispatch).
    pub fn handle_browser(
        &self,
        frame: &mut Frame,
        envelope: &Envelope,
        payload: &[u8],
        ctx: IpcContext,
        pending_clone: PendingMap,
    ) -> bool {
        match envelope.opcode {
            RPC_INVOKE => self.on_invoke(frame, envelope, payload, ctx, pending_clone),
            RPC_CANCEL => self.on_cancel(envelope, payload, ctx),
            _ => {
                debug!("[RPC Browser] unknown opcode {}", envelope.opcode);
                false
            }
        }
    }

    fn on_invoke(
        &self,
        frame: &mut Frame,
        envelope: &Envelope,
        payload: &[u8],
        ctx: IpcContext,
        pending_clone: PendingMap,
    ) -> bool {
        let (cmd, data_bytes) = match decode_cmd_payload(payload) {
            Some(v) => v,
            None => {
                debug!("[RPC Browser] invalid invoke payload");
                return false;
            }
        };

        let data_str = std::str::from_utf8(data_bytes).unwrap_or("");
        let id = envelope.correlation_id as i32;
        let correlation_id = envelope.correlation_id;
        debug!("[RPC Browser] invoke '{}' id={}", cmd, id);

        if self.is_async(cmd) {
            let aborted = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let browser_id = ctx.browser_id;
            if let Some(bid) = browser_id {
                pending_clone.insert(
                    bid,
                    id,
                    PendingEntry {
                        aborted: aborted.clone(),
                    },
                );
            }

            let responder = IpcResponder::new(Box::new({
                let aborted = aborted.clone();
                let frame = frame.clone();
                let pending = pending_clone.clone();
                move |result, error_code| {
                    if let Some(bid) = browser_id {
                        pending.remove(bid, id);
                    }
                    if !aborted.load(std::sync::atomic::Ordering::SeqCst) {
                        send_rpc_response(&frame, correlation_id, result, error_code);
                    } else {
                        debug!("[RPC Browser] dropping response for canceled id={}", id);
                    }
                }
            }));

            self.dispatch_async(cmd, data_str, responder, ctx);
        } else {
            let result = catch_unwind(AssertUnwindSafe(|| {
                self.dispatch(cmd, data_str, ctx)
            }));

            let (response, code) = match result {
                Ok(Ok(payload)) => (IpcResult::Ok(payload), 0),
                Ok(Err(msg)) => (IpcResult::Err(msg), 0),
                Err(_) => (IpcResult::Err("IPC handler panicked".to_string()), -1),
            };

            send_rpc_response(frame, correlation_id, response, code);
        }

        true
    }

    fn on_cancel(&self, envelope: &Envelope, _payload: &[u8], ctx: IpcContext) -> bool {
        let id = envelope.correlation_id as i32;
        if let Some(bid) = ctx.browser_id {
            self.pending.cancel(bid, id);
        }
        true
    }

    fn dispatch(&self, command: &str, payload_str: &str, ctx: IpcContext) -> IpcResult {
        match self.sync_handlers.get(command) {
            Some(h) => h(payload_str, ctx),
            None => Err(format!("unknown command '{command}'")),
        }
    }

    fn dispatch_async(&self, command: &str, payload_str: &str, responder: IpcResponder, ctx: IpcContext) {
        if let Some(handler) = self.async_handlers.get(command) {
            let value: serde_json::Value = if payload_str.is_empty() {
                serde_json::Value::Null
            } else {
                match serde_json::from_str(payload_str) {
                    Ok(v) => v,
                    Err(e) => {
                    responder.resolve(Err(format!("invalid JSON: {e}")), 0);
                    return;
                    }
                }
            };
            handler(value, responder, ctx);
        }
    }
}

/// Send an RPC response (resolve/reject) back to the renderer.
/// Reject payload: [error_code: i32 LE][error_message].
fn send_rpc_response(frame: &Frame, correlation_id: u32, result: IpcResult, error_code: i32) {
    if frame.is_valid() == 0 {
        debug!("[RPC Browser] frame destroyed, dropping id={}", correlation_id);
        return;
    }

    let (opcode, data) = match result {
        IpcResult::Ok(payload) => (RPC_RESOLVE, payload.into_bytes()),
        IpcResult::Err(err) => {
            let mut payload = Vec::with_capacity(4 + err.len());
            payload.extend_from_slice(&error_code.to_le_bytes());
            payload.extend_from_slice(err.as_bytes());
            (RPC_REJECT, payload)
        }
    };

    let envelope = Envelope {
        version: ENVELOPE_VERSION,
        subsystem: SUB_RPC,
        opcode,
        flags: 0,
        correlation_id,
        payload_kind: PAYLOAD_STRING,
    };

    if let Some(mut msg) = build_message("kurogane_rpc", &envelope, &data) {
        frame.send_process_message(ProcessId::RENDERER, Some(&mut msg));
    }
}
