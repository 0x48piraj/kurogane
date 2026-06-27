use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Arc;
use std::collections::HashMap;

use cef::*;

use crate::debug;
use crate::ipc::browser_state::IpcContext;
use crate::ipc::envelope::*;
use crate::ipc::pending::{PendingEntry, PendingMap};
use crate::ipc::transport::message::build_message;
use crate::ipc::responder::Responder;

pub type SyncHandler = Box<dyn Fn(&[u8], IpcContext) -> Result<Vec<u8>, String> + Send + Sync>;
pub type AsyncHandler = Box<dyn Fn(&[u8], BinaryResponder, IpcContext) + Send + Sync>;
pub type BinaryResponder = Responder<Vec<u8>>;

/// Unified request/response subsystem handling both JSON and Binary IPC.
pub struct RequestResponseSubsystem {
    pub sync_handlers: HashMap<String, SyncHandler>,
    pub async_handlers: HashMap<String, AsyncHandler>,
    pub pending: PendingMap,
}

impl RequestResponseSubsystem {
    pub fn new(
        sync_handlers: HashMap<String, SyncHandler>,
        async_handlers: HashMap<String, AsyncHandler>,
    ) -> Self {
        Self {
            sync_handlers,
            async_handlers,
            pending: PendingMap::new(),
        }
    }

    pub fn is_async(&self, command: &str) -> bool {
        self.async_handlers.contains_key(command)
    }

    pub fn is_sync(&self, command: &str) -> bool {
        self.sync_handlers.contains_key(command)
    }

    /// Handle a request/response message arriving from the renderer (browser-side dispatch).
    pub fn handle_browser(
        &self,
        frame: &mut Frame,
        envelope: &Envelope,
        payload: &[u8],
        ctx: IpcContext,
        pending_clone: PendingMap,
    ) -> bool {
        match envelope.opcode {
            0 => self.on_invoke(frame, envelope, payload, ctx, pending_clone),
            3 => self.on_cancel(envelope, payload, ctx),
            _ => {
                debug!("[RequestResponse Browser] unknown opcode {}", envelope.opcode);
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
        let (cmd, data) = match decode_cmd_payload(payload) {
            Some(v) => v,
            None => {
                debug!("[RequestResponse Browser] invalid invoke payload");
                return false;
            }
        };

        let id = envelope.correlation_id as i32;
        let correlation_id = envelope.correlation_id;
        debug!("[RequestResponse Browser] invoke '{}' id={}", cmd, id);

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
                let subsystem = envelope.subsystem;
                move |result, error_code| {
                    if let Some(bid) = browser_id {
                        pending.remove(bid, id);
                    }
                    if !aborted.load(std::sync::atomic::Ordering::SeqCst) {
                        send_response(&frame, subsystem, correlation_id, result, error_code);
                    } else {
                        debug!("[RequestResponse Browser] dropping response for canceled id={}", id);
                    }
                }
            }));

            self.dispatch_async(cmd, data, responder, ctx);
        } else {
            let result = catch_unwind(AssertUnwindSafe(|| {
                self.dispatch(cmd, data, ctx)
            }));

            let (response, code) = match result {
                Ok(Ok(payload)) => (Ok(payload), 0),
                Ok(Err(msg)) => (Err(msg), 0),
                Err(_) => (Err("handler panicked".to_string()), -1),
            };

            send_response(frame, envelope.subsystem, correlation_id, response, code);
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

    fn dispatch(&self, command: &str, data: &[u8], ctx: IpcContext) -> Result<Vec<u8>, String> {
        match self.sync_handlers.get(command) {
            Some(h) => h(data, ctx),
            None => Err(format!("unknown command '{command}'")),
        }
    }

    fn dispatch_async(&self, command: &str, data: &[u8], responder: BinaryResponder, ctx: IpcContext) {
        if let Some(handler) = self.async_handlers.get(command) {
            handler(data, responder, ctx);
        }
    }
}

fn send_response(frame: &Frame, subsystem: u8, correlation_id: u32, result: Result<Vec<u8>, String>, error_code: i32) {
    if frame.is_valid() == 0 {
        debug!("[RequestResponse Browser] frame destroyed, dropping id={}", correlation_id);
        return;
    }

    let (opcode_ok, opcode_err, payload_kind) = match subsystem {
        SUB_RPC => (RPC_RESOLVE, RPC_REJECT, PAYLOAD_STRING),
        SUB_BINARY => (BINARY_RESPONSE, BINARY_REJECT, PAYLOAD_BINARY),
        _ => return,
    };

    let (opcode, data) = match result {
        Ok(bytes) => (opcode_ok, bytes),
        Err(err) => {
            let mut payload = Vec::with_capacity(4 + err.len());
            payload.extend_from_slice(&error_code.to_le_bytes());
            payload.extend_from_slice(err.as_bytes());
            (opcode_err, payload)
        }
    };

    let envelope = Envelope {
        version: ENVELOPE_VERSION,
        subsystem,
        opcode,
        flags: 0,
        correlation_id,
        payload_kind,
    };

    if let Some(mut msg) = build_message("kurogane_rr", &envelope, &data) {
        frame.send_process_message(ProcessId::RENDERER, Some(&mut msg));
    } else {
        debug!("[RequestResponse Browser] failed to build response message");
    }
}
