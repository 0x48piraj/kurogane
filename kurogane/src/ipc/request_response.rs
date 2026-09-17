use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Arc;
use std::collections::HashMap;

use cef::*;

use crate::acl::Origin;
use crate::debug;
use crate::ipc::browser_state::{still_addressed, ErrorCode, IpcContext, IpcError};
use crate::ipc::envelope::*;
use crate::ipc::pending::{PendingEntry, PendingKey, PendingMap};
use crate::ipc::transport::message::build_message;
use crate::ipc::responder::Responder;

pub type SyncHandler = Box<dyn Fn(&[u8], IpcContext) -> Result<Vec<u8>, IpcError> + Send + Sync>;
pub type AsyncHandler = Box<dyn Fn(&[u8], BinaryResponder, IpcContext) + Send + Sync>;
pub type BinaryResponder = Responder<Vec<u8>>;

/// Unified request/response subsystem handling both JSON and Binary IPC.
///
/// Access control happens before dispatch, in the router.
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
            RPC_INVOKE => self.on_invoke(frame, envelope, payload, ctx, pending_clone),
            RPC_CANCEL => self.on_cancel(envelope, ctx),
            _ => {
                debug!(
                    "[RequestResponse Browser] unknown opcode {}",
                    envelope.opcode
                );
                false
            }
        }
    }

    /// Rejects the invocation with `error`, while the message is being
    /// handled (so the frame still shows its sender).
    pub(crate) fn reject(frame: &Frame, envelope: &Envelope, error: IpcError) {
        send_response(
            frame,
            envelope.payload_kind,
            envelope.correlation_id,
            Err(error),
            None,
        );
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

        let url_origin = ctx.url_origin.clone();
        if self.is_async(cmd) {
            let aborted = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let key = PendingKey::new(ctx.browser_id, ctx.frame.clone(), ctx.origin.clone(), id);
            pending_clone.insert(
                key.clone(),
                PendingEntry {
                    aborted: aborted.clone(),
                },
            );

            let responder = BinaryResponder::with_abort(
                Box::new({
                    let frame = frame.clone();
                    let pending = pending_clone.clone();
                    let payload_kind = envelope.payload_kind;
                    move |result| {
                        pending.remove(&key);
                        send_response(
                            &frame,
                            payload_kind,
                            correlation_id,
                            result,
                            Some(&url_origin),
                        );
                    }
                }),
                aborted,
            );

            self.dispatch_async(cmd, data, responder, ctx);
        } else {
            let result = catch_unwind(AssertUnwindSafe(|| self.dispatch(cmd, data, ctx)));

            let response = match result {
                Ok(res) => res,
                Err(_) => Err(IpcError::with_code("handler panicked", ErrorCode::Panic)),
            };

            send_response(
                frame,
                envelope.payload_kind,
                correlation_id,
                response,
                Some(&url_origin),
            );
        }

        true
    }

    /// Only the frame that sent a request, still showing the same origin, can
    /// cancel it; the pending entry is keyed by both.
    fn on_cancel(&self, envelope: &Envelope, ctx: IpcContext) -> bool {
        let key = PendingKey::new(
            ctx.browser_id,
            ctx.frame,
            ctx.origin,
            envelope.correlation_id as i32,
        );
        self.pending.cancel(&key);
        true
    }

    fn dispatch(&self, command: &str, data: &[u8], ctx: IpcContext) -> Result<Vec<u8>, IpcError> {
        match self.sync_handlers.get(command) {
            Some(h) => h(data, ctx),
            None => Err(IpcError::new(format!("unknown command '{command}'"))),
        }
    }

    fn dispatch_async(
        &self,
        command: &str,
        data: &[u8],
        responder: BinaryResponder,
        ctx: IpcContext,
    ) {
        if let Some(handler) = self.async_handlers.get(command) {
            // A panicking handler drops its responder, which rejects the
            // request; unwinding must not cross the CEF callback
            if catch_unwind(AssertUnwindSafe(|| handler(data, responder, ctx))).is_err() {
                debug!(
                    "[RequestResponse Browser] async handler '{}' panicked",
                    command
                );
            }
        }
    }
}

/// Sends a response to `frame`.
///
/// When `url_origin` is set, the response is dropped if the frame has been
/// destroyed or navigated to a different document.
fn send_response(
    frame: &Frame,
    payload_kind: u8,
    correlation_id: u32,
    result: Result<Vec<u8>, IpcError>,
    url_origin: Option<&Origin>,
) {
    if frame.is_valid() == 0 {
        debug!(
            "[RequestResponse Browser] frame destroyed, dropping id={}",
            correlation_id
        );
        return;
    }
    if let Some(url_origin) = url_origin {
        let url: CefString = (&frame.url()).into();
        if !still_addressed(url_origin, &url.to_string()) {
            debug!(
                "[RequestResponse Browser] frame shows another document, dropping id={}",
                correlation_id
            );
            return;
        }
    }

    let (opcode, data) = match result {
        Ok(bytes) => (RPC_RESOLVE, bytes),
        Err(err) => (
            RPC_REJECT,
            encode_error_payload(err.code().wire(), err.message()),
        ),
    };

    let envelope = Envelope {
        version: ENVELOPE_VERSION,
        subsystem: SUB_RPC,
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
