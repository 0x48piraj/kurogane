//! Browser-side stream dispatch.
//!
//! Manages the lifecycle of incoming renderer streams, including creation,
//! chunk delivery, completion, cancellation and cleanup of active streams.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use cef::*;

use crate::debug;
use crate::ipc::browser_state::IpcContext;
use crate::ipc::envelope::{Envelope, STREAM_OPEN, STREAM_DATA, STREAM_END, STREAM_ERROR, STREAM_CANCEL, decode_cmd_payload};
use crate::ipc::pending::PendingEntry;
use crate::ipc::stream::{StreamResponder, StreamSubsystem};

impl StreamSubsystem {
    /// Handle a stream message arriving from the renderer (browser-side dispatch).
    pub fn handle_browser(
        &self,
        frame: &mut Frame,
        envelope: &Envelope,
        payload: &[u8],
        ctx: IpcContext,
        pending_clone: crate::ipc::pending::PendingMap,
    ) -> bool {
        match envelope.opcode {
            STREAM_OPEN => self.on_open(frame, envelope, payload, ctx, pending_clone),
            STREAM_DATA => self.on_data(envelope, payload, ctx),
            STREAM_END => self.on_end(envelope, payload, ctx),
            STREAM_ERROR => self.on_error(envelope, payload, ctx),
            STREAM_CANCEL => self.on_cancel(envelope, ctx),
            _ => {
                debug!("[Stream Browser] unknown opcode {}", envelope.opcode);
                false
            }
        }
    }

    fn on_cancel(&self, envelope: &Envelope, ctx: IpcContext) -> bool {
        let id = envelope.correlation_id as i32;
        if let Some(bid) = ctx.browser_id {
            self.pending.cancel(bid, id);
        }
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
        pending_clone: crate::ipc::pending::PendingMap,
    ) -> bool {
        let (handler_name, _metadata) = match decode_cmd_payload(payload) {
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

        {
            let mut streams = self.streams.lock().unwrap();
            streams.insert(stream_id, (handler_name.to_string(), browser_id, frame.clone()));
        }

        debug!(
            "[Stream Browser] open '{}' stream_id={} browser={}",
            handler_name,
            stream_id,
            browser_id.as_u32()
        );
        true
    }

    fn on_data(
        &self,
        envelope: &Envelope,
        payload: &[u8],
        ctx: IpcContext,
    ) -> bool {
        let stream_id = envelope.correlation_id;
        let (handler_name, browser_id, frame) = {
            let streams = self.streams.lock().unwrap();
            match streams.get(&stream_id) {
                Some((h, b, f)) => (h.clone(), *b, f.clone()),
                None => {
                    debug!("[Stream Browser] data for unknown stream {}", stream_id);
                    return false;
                }
            }
        };

        if ctx.browser_id.map(|id| id != browser_id).unwrap_or(true) {
            debug!("[Stream Browser] browser mismatch for stream {}", stream_id);
            return false;
        }

        let handler = match self.handlers.get(&handler_name) {
            Some(h) => h,
            None => {
                debug!("[Stream Browser] no handler '{}' for stream data", handler_name);
                return false;
            }
        };

        let responder = StreamResponder::new(frame, stream_id);
        if let Err(e) = handler(stream_id, payload, false, &handler_name, responder, ctx) {
            debug!("[Stream Browser] stream handler error: {}", e);
        }

        true
    }

    fn on_end(
        &self,
        envelope: &Envelope,
        payload: &[u8],
        ctx: IpcContext,
    ) -> bool {
        let stream_id = envelope.correlation_id;

        let (handler_name, browser_id, frame) = {
            let mut streams = self.streams.lock().unwrap();
            match streams.remove(&stream_id) {
                Some(s) => s,
                None => {
                    debug!("[Stream Browser] end for unknown stream {}", stream_id);
                    return false;
                }
            }
        };

        if ctx.browser_id.map(|id| id != browser_id).unwrap_or(true) {
            debug!("[Stream Browser] browser mismatch for stream end {}", stream_id);
            return false;
        }

        // Remove pending entry
        if let Some(bid) = ctx.browser_id {
            self.pending.remove(bid, stream_id as i32);
        }

        let handler = match self.handlers.get(&handler_name) {
            Some(h) => h,
            None => {
                debug!("[Stream Browser] no handler '{}' for stream end", handler_name);
                return false;
            }
        };

        let responder = StreamResponder::new(frame, stream_id);
        if let Err(e) = handler(stream_id, payload, true, &handler_name, responder, ctx) {
            debug!("[Stream Browser] stream end handler error: {}", e);
        }

        debug!("[Stream Browser] end stream_id={}", stream_id);
        true
    }

    fn on_error(
        &self,
        envelope: &Envelope,
        payload: &[u8],
        ctx: IpcContext,
    ) -> bool {
        let stream_id = envelope.correlation_id;

        let (_handler_name, browser_id) = {
            let mut streams = self.streams.lock().unwrap();
            match streams.remove(&stream_id) {
                Some((h, b, _f)) => (h, b),
                None => {
                    debug!("[Stream Browser] error for unknown stream {}", stream_id);
                    return false;
                }
            }
        };

        if ctx.browser_id.map(|id| id != browser_id).unwrap_or(true) {
            debug!("[Stream Browser] browser mismatch for stream error {}", stream_id);
            return false;
        }

        // Remove pending entry
        if let Some(bid) = ctx.browser_id {
            self.pending.remove(bid, stream_id as i32);
        }

        let err_msg = String::from_utf8_lossy(payload).to_string();
        debug!("[Stream Browser] error stream_id={}: {}", stream_id, err_msg);
        true
    }
}
