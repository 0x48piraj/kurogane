use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Arc;

use cef::*;

use crate::debug;
use crate::ipc::browser_state::IpcContext;
use crate::ipc::envelope::*;
use crate::ipc::transport::message::build_message;
use crate::ipc::binary::{BinaryResponder, BinarySubsystem};
use crate::ipc::pending::{PendingEntry, PendingMap};

impl BinarySubsystem {
    /// Handle a binary message arriving from the renderer (browser-side dispatch).
    pub fn handle_browser(
        &self,
        frame: &mut Frame,
        envelope: &Envelope,
        payload: &[u8],
        ctx: IpcContext,
        pending_clone: PendingMap,
    ) -> bool {
        match envelope.opcode {
            BINARY_INVOKE => self.on_invoke(frame, envelope, payload, ctx, pending_clone),
            BINARY_CANCEL => self.on_cancel(envelope, payload, ctx),
            _ => {
                debug!("[Binary Browser] unknown opcode {}", envelope.opcode);
                false
            }
        }
    }

    fn on_cancel(&self, envelope: &Envelope, _payload: &[u8], ctx: IpcContext) -> bool {
        let id = envelope.correlation_id as i32;
        if let Some(bid) = ctx.browser_id {
            self.pending.cancel(bid, id);
        }
        true
    }

    fn on_invoke(
        &self,
        frame: &mut Frame,
        envelope: &Envelope,
        payload: &[u8],
        ctx: IpcContext,
        pending_clone: PendingMap,
    ) -> bool {
        let (cmd, data) = match decode_cmd_payload(payload) {
            Some(v) => v,
            None => {
                debug!("[Binary Browser] invalid invoke payload");
                return false;
            }
        };

        let id = envelope.correlation_id as i32;
        let correlation_id = envelope.correlation_id;
        debug!("[Binary Browser] invoke '{}' id={}", cmd, id);

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

            let responder = BinaryResponder::new(Box::new({
                let aborted = aborted.clone();
                let frame = frame.clone();
                let pending = pending_clone.clone();
                move |result, error_code| {
                    if let Some(bid) = browser_id {
                        pending.remove(bid, id);
                    }
                    if !aborted.load(std::sync::atomic::Ordering::SeqCst) {
                        send_binary_response(&frame, correlation_id, result, error_code);
                    } else {
                        debug!("[Binary Browser] dropping response for canceled id={}", id);
                    }
                }
            }));

            self.dispatch_async(cmd, data.to_vec(), responder, ctx);
        } else {
            let result = catch_unwind(AssertUnwindSafe(|| {
                self.dispatch(cmd, data, ctx)
            }));

            let (response, code) = match result {
                Ok(Ok(data)) => (Ok(data), 0),
                Ok(Err(msg)) => (Err(msg), 0),
                Err(_) => (Err("Binary handler panicked".to_string()), -1),
            };

            send_binary_response(frame, correlation_id, response, code);
        }

        true
    }

    fn dispatch(&self, command: &str, data: &[u8], ctx: IpcContext) -> Result<Vec<u8>, String> {
        match self.sync_handlers.get(command) {
            Some(h) => h(data, ctx),
            None => Err(format!("unknown binary command '{command}'")),
        }
    }

    fn dispatch_async(&self, command: &str, data: Vec<u8>, responder: BinaryResponder, ctx: IpcContext) {
        if let Some(handler) = self.async_handlers.get(command) {
            handler(data, responder, ctx);
        }
    }
}

/// Send a binary response back to the renderer.
/// On success: BINARY_RESPONSE with raw bytes.
/// On error: BINARY_REJECT with [error_code: i32 LE][error_message].
fn send_binary_response(frame: &Frame, correlation_id: u32, result: Result<Vec<u8>, String>, error_code: i32) {
    if frame.is_valid() == 0 {
        debug!("[Binary Browser] frame destroyed, dropping id={}", correlation_id);
        return;
    }

    let (opcode, data) = match result {
        Ok(bytes) => (BINARY_RESPONSE, bytes),
        Err(err) => {
            let mut payload = Vec::with_capacity(4 + err.len());
            payload.extend_from_slice(&error_code.to_le_bytes());
            payload.extend_from_slice(err.as_bytes());
            (BINARY_REJECT, payload)
        }
    };

    let envelope = Envelope {
        version: ENVELOPE_VERSION,
        subsystem: SUB_BINARY,
        opcode,
        flags: 0,
        correlation_id,
        payload_kind: PAYLOAD_BINARY,
    };

    if let Some(mut msg) = build_message("kurogane_binary", &envelope, &data) {
        frame.send_process_message(ProcessId::RENDERER, Some(&mut msg));
    } else {
        debug!("[Binary Browser] failed to build response message");
    }
}
