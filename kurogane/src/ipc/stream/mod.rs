//! Stream IPC subsystem.
//!
//! Provides bidirectional streaming data transport. Streams are
//! identified by a correlation ID and consist of open, data, end and error messages.

use std::collections::HashMap;
use cef::*;

use crate::browser_registry::BrowserId;
use crate::ipc::browser_state::IpcContext;
use crate::ipc::envelope::*;
use crate::ipc::transport::message::build_message;

/// Responder for sending data back to the renderer from the browser-side stream handler.
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

/// Application handler for an incoming stream.
///
/// Invoked for each data chunk and once more when the stream ends.
pub type StreamHandler =
    Box<dyn Fn(u32, &[u8], bool, &str, StreamResponder, IpcContext) -> Result<(), String> + Send + Sync>;

pub mod browser;
pub mod renderer;

/// Browser-side stream manager.
pub struct StreamSubsystem {
    pub handlers: HashMap<String, StreamHandler>,
    /// Track open streams per browser for cleanup
    pub streams: std::sync::Mutex<HashMap<u32, (String, BrowserId, Frame)>>,
    pub pending: crate::ipc::pending::PendingMap,
}

impl StreamSubsystem {
    pub fn new(handlers: HashMap<String, StreamHandler>) -> Self {
        Self {
            handlers,
            streams: std::sync::Mutex::new(HashMap::new()),
            pending: crate::ipc::pending::PendingMap::new(),
        }
    }

    /// Remove all streams for a given browser.
    pub fn clear_for_browser(&self, browser_id: BrowserId) -> usize {
        let mut streams = self.streams.lock().unwrap();
        let before = streams.len();
        streams.retain(|_, (_, bid, _)| *bid != browser_id);
        before - streams.len()
    }
}
