//! Stream IPC subsystem.
//!
//! Provides bidirectional streaming data transport. Streams are
//! identified by a correlation ID and consist of open, data, end and error messages.
//!
//! Each stream gets its own handler instance via a factory closure,
//! giving handlers natural per-stream mutable state.

use std::collections::HashMap;
use std::sync::Mutex;
use cef::*;

use crate::acl::Origin;
use crate::browser_registry::BrowserId;
use crate::ipc::envelope::*;
use crate::ipc::{ErrorCode, FrameId, IpcContext};
use crate::ipc::transport::message::build_message;

/// Responder for sending data back to the renderer from the browser-side stream handler.
#[derive(Clone)]
pub struct StreamResponder {
    frame: Frame,
    stream_id: u32,
}

impl StreamResponder {
    pub fn new(frame: Frame, stream_id: u32) -> Self {
        Self { frame, stream_id }
    }

    /// Send a data chunk to the renderer.
    pub fn send_data(&self, data: &[u8]) -> Result<(), String> {
        if self.frame.is_valid() == 0 {
            return Err("frame destroyed".into());
        }
        let envelope = Envelope {
            version: ENVELOPE_VERSION,
            subsystem: SUB_STREAM,
            opcode: STREAM_BROWSER_DATA,
            flags: 0,
            correlation_id: self.stream_id,
            payload_kind: PAYLOAD_BINARY,
        };
        let mut msg = build_message("kurogane_stream", &envelope, data)
            .ok_or_else(|| "failed to build STREAM_BROWSER_DATA message".to_string())?;
        self.frame
            .send_process_message(ProcessId::RENDERER, Some(&mut msg));
        Ok(())
    }

    /// Signal that the browser is done sending data for this stream.
    pub fn end(&self, result: &str) -> Result<(), String> {
        if self.frame.is_valid() == 0 {
            return Err("frame destroyed".into());
        }
        let envelope = Envelope {
            version: ENVELOPE_VERSION,
            subsystem: SUB_STREAM,
            opcode: STREAM_BROWSER_END,
            flags: 0,
            correlation_id: self.stream_id,
            payload_kind: PAYLOAD_STRING,
        };
        let payload = result.as_bytes();
        let mut msg = build_message("kurogane_stream", &envelope, payload)
            .ok_or_else(|| "failed to build STREAM_BROWSER_END message".to_string())?;
        self.frame
            .send_process_message(ProcessId::RENDERER, Some(&mut msg));
        Ok(())
    }

    /// Signal an error to the renderer for this stream.
    pub fn error(&self, msg: &str) -> Result<(), String> {
        self.error_with_code(msg, ErrorCode::Handler)
    }

    /// Signal an error of class `code`: the renderer rejects with that code.
    pub(crate) fn error_with_code(&self, msg: &str, code: ErrorCode) -> Result<(), String> {
        if self.frame.is_valid() == 0 {
            return Err("frame destroyed".into());
        }
        let envelope = Envelope {
            version: ENVELOPE_VERSION,
            subsystem: SUB_STREAM,
            opcode: STREAM_BROWSER_ERROR,
            flags: 0,
            correlation_id: self.stream_id,
            payload_kind: PAYLOAD_BINARY,
        };
        let payload = encode_error_payload(code.wire(), msg);
        let mut msg = build_message("kurogane_stream", &envelope, &payload)
            .ok_or_else(|| "failed to build STREAM_BROWSER_ERROR message".to_string())?;
        self.frame
            .send_process_message(ProcessId::RENDERER, Some(&mut msg));
        Ok(())
    }
}

/// Per-stream handler trait.
///
/// Implement this trait to handle a stream's full lifecycle.
/// The framework instantiates the handler via a factory closure
/// when a stream opens and drops it when the stream ends or errors.
///
/// Each callback receives a StreamResponder so handlers can send data
/// back to the renderer without storing the responder themselves.
///
/// on_chunk borrows the responder (the stream continues).
/// on_end takes ownership (the stream is consumed).
pub trait StreamHandler: Send + 'static {
    /// Called when the stream opens.
    fn on_open(&mut self, metadata: &str, responder: &StreamResponder) -> Result<(), String> {
        let _ = (metadata, responder);
        Ok(())
    }

    /// Called for each data chunk from the renderer.
    fn on_chunk(&mut self, data: &[u8], responder: &StreamResponder) -> Result<(), String>;

    /// Called when the renderer closes the stream normally.
    fn on_end(&mut self, result: &str, responder: StreamResponder) -> Result<(), String> {
        let _ = (result, responder);
        Ok(())
    }

    /// Called if the stream errors.
    fn on_error(&mut self, message: &str) {
        let _ = message;
    }
}

/// Factory type: creates a new handler instance per stream.
pub type StreamFactory = Box<dyn Fn() -> Box<dyn StreamHandler> + Send + Sync>;

pub mod browser;
pub mod renderer;

type StreamEntry = (BrowserId, Box<dyn StreamHandler>, Frame);

/// A stream's identity: the frame and origin that opened it plus its stream
/// id. Stream ids are allocated per renderer process, so the frame keeps two
/// frames' streams apart, and the origin keeps a later document in the same
/// frame away from a stream an earlier one opened. Only a message from the
/// opening frame, still showing the same origin, can feed, end or cancel one.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct StreamKey {
    frame: FrameId,
    origin: Origin,
    id: u32,
}

impl StreamKey {
    pub(crate) fn new(ctx: &IpcContext, id: u32) -> Self {
        Self {
            frame: ctx.frame.clone(),
            origin: ctx.origin.clone(),
            id,
        }
    }
}

/// Browser-side stream manager.
pub struct StreamSubsystem {
    pub factories: HashMap<String, StreamFactory>,
    /// Per-stream handler instances, keyed by opening frame and stream id.
    /// Stores the frame alongside each handler so responders can be
    /// reconstructed on every callback instead of stored by the handler.
    pub streams: Mutex<HashMap<StreamKey, StreamEntry>>,
}

impl StreamSubsystem {
    pub fn new(factories: HashMap<String, StreamFactory>) -> Self {
        Self {
            factories,
            streams: Mutex::new(HashMap::new()),
        }
    }

    /// Remove the streams of `frame`, which is starting a new document.
    pub fn clear_frame(&self, frame: &FrameId) -> usize {
        let mut streams = self.streams.lock().unwrap();
        let before = streams.len();
        streams.retain(|key, _| key.frame != *frame);
        before - streams.len()
    }

    /// Remove all streams whose frame is no longer valid.
    pub fn clear_invalid_frames(&self) -> usize {
        let mut streams = self.streams.lock().unwrap();
        let before = streams.len();
        streams.retain(|_, (_, _, frame)| frame.is_valid() != 0);
        before - streams.len()
    }

    /// Remove all streams for a given browser.
    pub fn clear_for_browser(&self, browser_id: BrowserId) -> usize {
        let mut streams = self.streams.lock().unwrap();
        let before = streams.len();
        streams.retain(|_, (bid, _, _)| *bid != browser_id);
        before - streams.len()
    }
}
