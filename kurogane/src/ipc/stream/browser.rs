//! Browser-side stream dispatch.
//!
//! Manages the lifecycle of incoming renderer streams, including creation,
//! chunk delivery, completion, cancellation and cleanup of active streams.
//! Each stream gets its own handler instance from the registered factory.
//!
//! The frame is stored alongside each handler so StreamResponders can be
//! reconstructed on every callback, eliminating the need for handlers to
//! store an Option<StreamResponder> themselves.

use cef::*;
use std::panic::{catch_unwind, AssertUnwindSafe};

use crate::debug;
use crate::ipc::browser_state::IpcContext;
use crate::ipc::envelope::{Envelope, STREAM_OPEN, STREAM_DATA, STREAM_END, STREAM_ERROR, STREAM_CANCEL, decode_cmd_payload};
use crate::ipc::stream::{StreamResponder, StreamSubsystem};

impl StreamSubsystem {
    /// Handle a stream message arriving from the renderer (browser-side dispatch).
    pub fn handle_browser(
        &self,
        frame: &mut Frame,
        envelope: &Envelope,
        payload: &[u8],
        ctx: IpcContext,
    ) -> bool {
        match envelope.opcode {
            STREAM_OPEN => self.on_open(frame, envelope, payload, ctx),
            STREAM_DATA => self.on_data(envelope, payload),
            STREAM_END => self.on_end(envelope, payload),
            STREAM_ERROR => self.on_error(envelope, payload),
            STREAM_CANCEL => self.on_cancel(envelope),
            _ => {
                debug!("[Stream Browser] unknown opcode {}", envelope.opcode);
                false
            }
        }
    }

    fn on_cancel(&self, envelope: &Envelope) -> bool {
        let stream_id = envelope.correlation_id;
        self.streams.lock().unwrap().remove(&stream_id);
        true
    }

    fn on_open(
        &self,
        frame: &mut Frame,
        envelope: &Envelope,
        payload: &[u8],
        ctx: IpcContext,
    ) -> bool {
        let stream_id = envelope.correlation_id;

        let (handler_name, metadata_bytes) = match decode_cmd_payload(payload) {
            Some(v) => v,
            None => {
                debug!("[Stream Browser] invalid open payload");
                let responder = StreamResponder::new(frame.clone(), stream_id);
                let _ = responder.error("invalid open payload");
                return false;
            }
        };

        let browser_id = match ctx.browser_id {
            Some(id) => id,
            None => {
                debug!("[Stream Browser] open without browser_id");
                let responder = StreamResponder::new(frame.clone(), stream_id);
                let _ = responder.error("no browser_id");
                return false;
            }
        };

        let factory = match self.factories.get(handler_name) {
            Some(f) => f,
            None => {
                debug!("[Stream Browser] no factory '{}' for stream open", handler_name);
                let responder = StreamResponder::new(frame.clone(), stream_id);
                let _ = responder.error(&format!("no handler registered for '{handler_name}'"));
                return false;
            }
        };

        let mut handler = factory();

        // Pass responder by reference
        let responder = StreamResponder::new(frame.clone(), stream_id);

        let metadata_str = std::str::from_utf8(metadata_bytes).unwrap_or("");
        let open_result = catch_unwind(AssertUnwindSafe(|| {
            handler.on_open(metadata_str, &responder)
        }));

        match open_result {
            Ok(Ok(())) => {
                // Retain the handler and frame for subsequent stream callbacks
                {
                    let mut streams = self.streams.lock().unwrap();
                    streams.insert(stream_id, (browser_id, handler, frame.clone()));
                }

                // Complete the open handshake by acknowledging success
                let _ = responder.end("");

                debug!(
                    "[Stream Browser] open '{}' stream_id={}",
                    handler_name, stream_id,
                );
                true
            }
            Ok(Err(e)) => {
                debug!("[Stream Browser] on_open error: {}", e);
                let _ = responder.error(&e);
                false
            }
            Err(_) => {
                debug!("[Stream Browser] on_open handler panicked");
                let _ = responder.error("handler panicked");
                false
            }
        }
    }

    fn on_data(&self, envelope: &Envelope, payload: &[u8]) -> bool {
        let stream_id = envelope.correlation_id;

        let entry = self.streams.lock().unwrap().remove(&stream_id);
        let Some((browser_id, mut handler, frame)) = entry else {
            debug!("[Stream Browser] data for unknown stream {}", stream_id);
            return false;
        };

        // Reconstruct responder from the stored frame, the handler never needs to store one itself
        let responder = StreamResponder::new(frame.clone(), stream_id);
        let data_result = catch_unwind(AssertUnwindSafe(|| {
            handler.on_chunk(payload, &responder)
        }));

        match data_result {
            Ok(Ok(())) => {
                self.streams.lock().unwrap().insert(stream_id, (browser_id, handler, frame));
            }
            Ok(Err(e)) => {
                debug!("[Stream Browser] on_chunk error: {}", e);
                let _ = responder.error(&e);
            }
            Err(_) => {
                debug!("[Stream Browser] on_chunk handler panicked");
                let _ = responder.error("handler panicked");
            }
        }

        true
    }

    fn on_end(&self, envelope: &Envelope, payload: &[u8]) -> bool {
        let stream_id = envelope.correlation_id;
        let result_str = String::from_utf8_lossy(payload).to_string();

        // Remove entry, take ownership of handler and frame
        let entry = self.streams.lock().unwrap().remove(&stream_id);
        if let Some((_, mut handler, frame)) = entry {
            let responder = StreamResponder::new(frame, stream_id);
            let responder_clone = responder.clone();
            let end_result = catch_unwind(AssertUnwindSafe(|| {
                handler.on_end(&result_str, responder)
            }));

            match end_result {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    debug!("[Stream Browser] on_end error: {}", e);
                    let _ = responder_clone.error(&e);
                }
                Err(_) => {
                    debug!("[Stream Browser] on_end handler panicked");
                    let _ = responder_clone.error("handler panicked");
                }
            }
        } else {
            debug!("[Stream Browser] end for unknown stream {}", stream_id);
            return false;
        }

        debug!("[Stream Browser] end stream_id={}", stream_id);
        true
    }

    fn on_error(&self, envelope: &Envelope, payload: &[u8]) -> bool {
        let stream_id = envelope.correlation_id;
        let err_msg = String::from_utf8_lossy(payload).to_string();

        let entry = self.streams.lock().unwrap().remove(&stream_id);
        if let Some((_, mut handler, _)) = entry {
            handler.on_error(&err_msg);
        } else {
            debug!("[Stream Browser] error for unknown stream {}", stream_id);
            return false;
        }

        debug!("[Stream Browser] error stream_id={}: {}", stream_id, err_msg);
        true
    }
}
