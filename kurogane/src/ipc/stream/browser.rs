//! Browser-side stream dispatch.
//!
//! Manages the lifecycle of incoming renderer streams, including creation,
//! chunk delivery, completion, cancellation and cleanup of active streams.
//! Each stream gets its own handler instance from the registered factory.
//!
//! An open is answered before the handler can send: `STREAM_BROWSER_OPENED`
//! once `on_open` accepts, or `STREAM_BROWSER_ERROR`.
//!
//! Streams are scoped to the frame and origin that opened them. Only that
//! frame may send data, complete, or cancel the stream and only while it
//! retains the same origin.
//!
//! Stream responders share stream state, so responses stop once the stream
//! has ended or failed.

use cef::*;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use crate::debug;
use crate::ipc::browser_state::{ErrorCode, IpcContext};
use crate::ipc::envelope::{
    Envelope, STREAM_OPEN, STREAM_DATA, STREAM_END, STREAM_ERROR, STREAM_CANCEL, decode_cmd_payload,
};
use crate::ipc::stream::{StreamEntry, StreamKey, StreamResponder, StreamSubsystem};
use crate::runtime::AppHandle;

impl StreamSubsystem {
    /// Handle a stream message arriving from the renderer (browser-side dispatch).
    pub fn handle_browser(
        &self,
        app: &AppHandle,
        frame: &mut Frame,
        envelope: &Envelope,
        payload: &[u8],
        ctx: IpcContext,
    ) -> bool {
        let key = StreamKey::new(&ctx, envelope.correlation_id);
        match envelope.opcode {
            STREAM_OPEN => self.on_open(app, frame, key, payload, ctx),
            STREAM_DATA => self.on_data(key, payload),
            STREAM_END => self.on_end(key, payload),
            STREAM_ERROR => self.on_error(key, payload),
            STREAM_CANCEL => self.on_cancel(&key),
            _ => {
                debug!("[Stream Browser] unknown opcode {}", envelope.opcode);
                false
            }
        }
    }

    fn on_cancel(&self, key: &StreamKey) -> bool {
        if let Some(entry) = self.streams.lock().unwrap().remove(key) {
            entry.close();
        }
        true
    }

    fn on_open(
        &self,
        app: &AppHandle,
        frame: &mut Frame,
        key: StreamKey,
        payload: &[u8],
        ctx: IpcContext,
    ) -> bool {
        // Streams another thread ended or failed are over: drop their handlers
        self.close_where(|_, entry| entry.is_closed());

        let stream_id = key.id;
        let closed = Arc::new(AtomicBool::new(false));
        let responder = StreamResponder::bound(
            frame.clone(),
            stream_id,
            ctx.url_origin.clone(),
            closed.clone(),
        );

        let (handler_name, metadata_bytes) = match decode_cmd_payload(payload) {
            Some(v) => v,
            None => {
                debug!("[Stream Browser] invalid open payload");
                let _ = responder.error_with_code("invalid open payload", ErrorCode::Buffer);
                return false;
            }
        };

        let Ok(metadata) = std::str::from_utf8(metadata_bytes) else {
            debug!("[Stream Browser] open metadata is not UTF-8");
            let _ = responder.error_with_code("stream metadata is not UTF-8", ErrorCode::Buffer);
            return false;
        };

        let factory = match self.factories.get(handler_name) {
            Some(f) => f,
            None => {
                debug!(
                    "[Stream Browser] no factory '{}' for stream open",
                    handler_name
                );
                let _ = responder.error(&format!("no handler registered for '{handler_name}'"));
                return false;
            }
        };

        let Ok(mut handler) = catch_unwind(AssertUnwindSafe(|| factory(app))) else {
            debug!("[Stream Browser] factory '{}' panicked", handler_name);
            let _ = responder.error("handler panicked");
            return false;
        };

        // The handler accepts or refuses the stream before the page holds it
        match catch_unwind(AssertUnwindSafe(|| handler.on_open(metadata))) {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                debug!("[Stream Browser] on_open error: {}", e);
                let _ = responder.error(&e);
                return false;
            }
            Err(_) => {
                debug!("[Stream Browser] on_open handler panicked");
                let _ = responder.error("handler panicked");
                return false;
            }
        }

        // The page learns the stream is open before anything else of it
        if let Err(e) = responder.opened() {
            debug!(
                "[Stream Browser] open '{}' not acknowledged: {}",
                handler_name, e
            );
            return false;
        }
        debug!(
            "[Stream Browser] open '{}' stream_id={}",
            handler_name, stream_id,
        );

        let mut entry = StreamEntry {
            browser_id: ctx.browser_id,
            handler,
            frame: frame.clone(),
            url_origin: ctx.url_origin,
            closed,
        };
        let opened = catch_unwind(AssertUnwindSafe(|| entry.handler.on_opened(&responder)));
        let failure = match opened {
            Ok(Ok(())) => None,
            Ok(Err(e)) => Some(e),
            Err(_) => Some("handler panicked".to_owned()),
        };
        if let Some(e) = &failure {
            debug!("[Stream Browser] on_opened error: {}", e);
            let _ = responder.error(e);
        }
        if failure.is_some() || entry.is_closed() {
            // Over already: a responder the handler kept stops here too
            entry.close();
            return true;
        }

        // Retain the handler for the stream's later messages
        self.streams.lock().unwrap().insert(key, entry);
        true
    }

    fn on_data(&self, key: StreamKey, payload: &[u8]) -> bool {
        let stream_id = key.id;

        let entry = self.streams.lock().unwrap().remove(&key);
        let Some(mut entry) = entry else {
            debug!("[Stream Browser] data for unknown stream {}", stream_id);
            return false;
        };
        if entry.is_closed() {
            // Another thread ended or failed the stream while this chunk was on its way
            return true;
        }

        let responder = entry.responder(stream_id);
        let data_result = catch_unwind(AssertUnwindSafe(|| {
            entry.handler.on_chunk(payload, &responder)
        }));

        match data_result {
            // Kept for the next chunk, unless the handler ended or failed the stream
            Ok(Ok(())) if !entry.is_closed() => {
                self.streams.lock().unwrap().insert(key, entry);
            }
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                debug!("[Stream Browser] on_chunk error: {}", e);
                let _ = responder.error(&e);
                entry.close();
            }
            Err(_) => {
                debug!("[Stream Browser] on_chunk handler panicked");
                let _ = responder.error("handler panicked");
                entry.close();
            }
        }

        true
    }

    fn on_end(&self, key: StreamKey, payload: &[u8]) -> bool {
        let stream_id = key.id;
        let result_str = String::from_utf8_lossy(payload).to_string();

        // Remove the entry and take ownership of its handler
        let entry = self.streams.lock().unwrap().remove(&key);
        let Some(mut entry) = entry else {
            debug!("[Stream Browser] end for unknown stream {}", stream_id);
            return false;
        };
        if entry.is_closed() {
            // Another thread ended or failed the stream: the page has its answer
            return true;
        }
        let responder = entry.responder(stream_id);
        let responder_clone = responder.clone();
        let end_result = catch_unwind(AssertUnwindSafe(|| {
            entry.handler.on_end(&result_str, responder)
        }));

        match end_result {
            // The page's end is answered: by the handler's own end or error
            // if it sent one, otherwise with an empty end
            Ok(Ok(())) => {
                let _ = responder_clone.end("");
            }
            Ok(Err(e)) => {
                debug!("[Stream Browser] on_end error: {}", e);
                let _ = responder_clone.error(&e);
            }
            Err(_) => {
                debug!("[Stream Browser] on_end handler panicked");
                let _ = responder_clone.error("handler panicked");
            }
        }
        // The stream is consumed: a responder the handler kept stops here
        entry.close();

        debug!("[Stream Browser] end stream_id={}", stream_id);
        true
    }

    fn on_error(&self, key: StreamKey, payload: &[u8]) -> bool {
        let stream_id = key.id;
        let err_msg = String::from_utf8_lossy(payload).to_string();

        let entry = self.streams.lock().unwrap().remove(&key);
        let Some(mut entry) = entry else {
            debug!("[Stream Browser] error for unknown stream {}", stream_id);
            return false;
        };
        entry.close();
        if catch_unwind(AssertUnwindSafe(|| entry.handler.on_error(&err_msg))).is_err() {
            debug!("[Stream Browser] on_error handler panicked");
        }

        debug!(
            "[Stream Browser] error stream_id={}: {}",
            stream_id, err_msg
        );
        true
    }
}
