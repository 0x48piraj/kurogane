use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Arc;
use std::collections::HashMap;

use tanso::*;

use tracing::debug;
use crate::ipc::browser_outbox;
use crate::ipc::browser_state::{ErrorCode, FrameId, IpcContext, IpcError};
use crate::ipc::document::Document;
use crate::ipc::envelope::*;
use crate::ipc::pending::{PendingEntry, PendingKey, PendingMap};
use crate::ipc::responder::Responder;
use crate::runtime::AppHandle;

pub type SyncHandler =
    Box<dyn Fn(&[u8], &AppHandle, IpcContext) -> Result<Vec<u8>, IpcError> + Send + Sync>;
pub type AsyncHandler = Box<dyn Fn(&[u8], BinaryResponder, &AppHandle, IpcContext) + Send + Sync>;
/// Responder of an async binary command, resolved with the reply's bytes
/// ([`App::async_binary_command`](crate::App::async_binary_command)).
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
        app: &AppHandle,
        frame: &mut Frame,
        envelope: &Envelope,
        payload: &[u8],
        ctx: IpcContext,
    ) -> bool {
        match envelope.opcode {
            RPC_INVOKE => self.on_invoke(app, frame, envelope, payload, ctx),
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
        reply(
            frame,
            &FrameId::of(frame),
            envelope.payload_kind,
            envelope.correlation_id,
            Err(error),
            None,
        );
    }

    fn on_invoke(
        &self,
        app: &AppHandle,
        frame: &mut Frame,
        envelope: &Envelope,
        payload: &[u8],
        ctx: IpcContext,
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

        let document = ctx.document.clone();
        if self.is_async(cmd) {
            let aborted = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let key = PendingKey::new(document.key(), id);
            self.pending.insert(
                key,
                PendingEntry {
                    aborted: aborted.clone(),
                },
            );

            let responder = BinaryResponder::with_abort(
                Box::new({
                    let frame = frame.clone();
                    let pending = self.pending.clone();
                    let payload_kind = envelope.payload_kind;
                    move |result| {
                        pending.remove(&key);
                        reply(
                            &frame,
                            document.frame(),
                            payload_kind,
                            correlation_id,
                            result,
                            Some(&document),
                        );
                    }
                }),
                aborted,
            );

            self.dispatch_async(app, cmd, data, responder, ctx);
        } else {
            let result = catch_unwind(AssertUnwindSafe(|| self.dispatch(app, cmd, data, ctx)));

            let response = match result {
                Ok(res) => res,
                Err(_) => Err(IpcError::with_code("handler panicked", ErrorCode::Panic)),
            };

            // Made while the browser handles the message, which the document
            // sent
            reply(
                frame,
                document.frame(),
                envelope.payload_kind,
                correlation_id,
                response,
                None,
            );
        }

        true
    }

    /// Only the document that sent a request can cancel it; the pending
    /// entry is keyed by it.
    fn on_cancel(&self, envelope: &Envelope, ctx: IpcContext) -> bool {
        let key = PendingKey::new(ctx.document.key(), envelope.correlation_id as i32);
        self.pending.cancel(&key);
        true
    }

    fn dispatch(
        &self,
        app: &AppHandle,
        command: &str,
        data: &[u8],
        ctx: IpcContext,
    ) -> Result<Vec<u8>, IpcError> {
        match self.sync_handlers.get(command) {
            Some(h) => h(data, app, ctx),
            None => Err(IpcError::new(format!("unknown command '{command}'"))),
        }
    }

    fn dispatch_async(
        &self,
        app: &AppHandle,
        command: &str,
        data: &[u8],
        responder: BinaryResponder,
        ctx: IpcContext,
    ) {
        if let Some(handler) = self.async_handlers.get(command) {
            // A panicking handler drops its responder, which rejects the
            // request; unwinding must not cross the CEF callback
            if catch_unwind(AssertUnwindSafe(|| handler(data, responder, app, ctx))).is_err() {
                debug!(
                    "[RequestResponse Browser] async handler '{}' panicked",
                    command
                );
            }
        }
    }
}

/// Answers a call from `frame`. With `document` set the reply is dropped
/// once that document has ended.
fn reply(
    frame: &Frame,
    frame_id: &FrameId,
    payload_kind: u8,
    correlation_id: u32,
    result: Result<Vec<u8>, IpcError>,
    document: Option<&Document>,
) {
    if document.is_some_and(|document| !document.is_live()) {
        debug!(
            "[RequestResponse Browser] the document has ended, dropping id={}",
            correlation_id
        );
        return;
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

    browser_outbox::send_owned(frame, frame_id, &envelope, data);
}
