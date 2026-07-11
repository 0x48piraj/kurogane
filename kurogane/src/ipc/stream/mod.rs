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

use crate::browser_registry::BrowserId;
use crate::ipc::envelope::*;
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
        self.frame.send_process_message(ProcessId::RENDERER, Some(&mut msg));
        Ok(())
    }

    /// Signal that the browser is done sending data for this stream.
    pub fn end(&self, result: &str) -> Result<(), String> {
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
        self.frame.send_process_message(ProcessId::RENDERER, Some(&mut msg));
        Ok(())
    }

    /// Signal an error to the renderer for this stream.
    pub fn error(&self, msg: &str) -> Result<(), String> {
        let envelope = Envelope {
            version: ENVELOPE_VERSION,
            subsystem: SUB_STREAM,
            opcode: STREAM_BROWSER_ERROR,
            flags: 0,
            correlation_id: self.stream_id,
            payload_kind: PAYLOAD_STRING,
        };
        let payload = msg.as_bytes();
        let mut msg = build_message("kurogane_stream", &envelope, payload)
            .ok_or_else(|| "failed to build STREAM_BROWSER_ERROR message".to_string())?;
        self.frame.send_process_message(ProcessId::RENDERER, Some(&mut msg));
        Ok(())
    }
}

/// Per-stream handler trait.
///
/// Implement this trait to handle a stream's full lifecycle.
/// The framework instantiates the handler via a factory closure
/// when a stream opens, and drops it when the stream ends or errors.
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

/// Browser-side stream manager.
pub struct StreamSubsystem {
    pub factories: HashMap<String, StreamFactory>,
    /// Per-stream handler instances, keyed by stream_id.
    /// Stores the frame alongside each handler so responders can be
    /// reconstructed on every callback instead of stored by the handler.
    pub streams: Mutex<HashMap<u32, StreamEntry>>,
}

impl StreamSubsystem {
    pub fn new(factories: HashMap<String, StreamFactory>) -> Self {
        Self {
            factories,
            streams: Mutex::new(HashMap::new()),
        }
    }

    /// Remove all streams for a given browser.
    pub fn clear_for_browser(&self, browser_id: BrowserId) -> usize {
        let mut streams = self.streams.lock().unwrap();
        let before = streams.len();
        streams.retain(|_, (bid, _, _)| *bid != browser_id);
        before - streams.len()
    }
}
