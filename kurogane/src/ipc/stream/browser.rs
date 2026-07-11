//! Browser-side stream dispatch.
//!
//! Manages the lifecycle of incoming renderer streams, including creation,
//! chunk delivery, completion, cancellation and cleanup of active streams.
//! Each stream gets its own handler instance from the registered factory.
//!
//! The frame is stored alongside each handler so StreamResponders can be
//! reconstructed on every callback, eliminating the need for handlers to
//! store an Option<StreamResponder> themselves.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use cef::*;

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
        let (handler_name, metadata_bytes) = match decode_cmd_payload(payload) {
            Some(v) => v,
            None => {
                debug!("[Stream Browser] invalid open payload");
                return false;
            }
        };

        let stream_id = envelope.correlation_id;
        let browser_id = match ctx.browser_id {
            Some(id) => id,
            None => {
                debug!("[Stream Browser] open without browser_id");
                return false;
            }
        };

        // Register pending entry so the stream can be cancelled on browser destroy
        pending_clone.insert(
            browser_id,
            stream_id as i32,
            PendingEntry {
                aborted: Arc::new(AtomicBool::new(false)),
            },
        );

        let factory = match self.factories.get(handler_name) {
            Some(f) => f,
            None => {
                debug!("[Stream Browser] no factory '{}' for stream open", handler_name);
                return false;
            }
        };

        let mut handler = factory();

        // Pass responder by reference
        let responder = StreamResponder::new(frame.clone(), stream_id);

        let metadata_str = std::str::from_utf8(metadata_bytes).unwrap_or("");
        if let Err(e) = handler.on_open(metadata_str, &responder) {
            debug!("[Stream Browser] on_open error: {}", e);
            return false;
        }

        // Store Frame alongside handler so responders can be reconstructed
        // for every subsequent callback without the handler storing one.
        {
            let mut streams = self.streams.lock().unwrap();
            streams.insert(stream_id, (browser_id, handler, frame.clone()));
        }

        debug!(
            "[Stream Browser] open '{}' stream_id={}",
            handler_name, stream_id,
        );
        true
    }

    fn on_data(&self, envelope: &Envelope, payload: &[u8]) -> bool {
        let stream_id = envelope.correlation_id;

        let mut streams = self.streams.lock().unwrap();
        let (_, handler, frame) = match streams.get_mut(&stream_id) {
            Some(entry) => entry,
            None => {
                debug!("[Stream Browser] data for unknown stream {}", stream_id);
                return false;
            }
        };

        // Reconstruct responder from the stored frame, the handler never
        // needs to store one itself.
        let responder = StreamResponder::new(frame.clone(), stream_id);
        if let Err(e) = handler.on_chunk(payload, &responder) {
            debug!("[Stream Browser] on_chunk error: {}", e);
            // Propagate error back to renderer
            let _ = responder.error(&e);
            drop(streams);
            self.streams.lock().unwrap().remove(&stream_id);
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
            if let Err(e) = handler.on_end(&result_str, responder) {
                debug!("[Stream Browser] on_end error: {}", e);
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
