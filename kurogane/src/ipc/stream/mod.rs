//! Stream IPC subsystem.
//!
//! Provides bidirectional streaming data transport. Streams are
//! identified by a correlation ID and consist of open, data, end and error messages.
//!
//! Each stream gets its own handler instance via a factory closure,
//! giving handlers natural per-stream mutable state.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use cef::*;

use crate::acl::Origin;
use crate::browser_registry::BrowserId;
use crate::ipc::browser_state::still_addressed;
use crate::ipc::envelope::*;
use crate::ipc::{ErrorCode, FrameId, IpcContext};
use crate::ipc::transport::message::build_message;

/// Responder for sending data from a browser-side stream handler to the renderer.
///
/// The responder is bound to its stream and stops sending when the stream is
/// closed or its document is replaced.
#[derive(Clone)]
pub struct StreamResponder {
    frame: Frame,
    stream_id: u32,
    /// URL origin of the document that opened the stream. Responses are sent only
    /// while the frame still shows that origin.
    url_origin: Option<Origin>,
    /// Shared stream state.
    closed: Arc<AtomicBool>,
}

impl StreamResponder {
    pub fn new(frame: Frame, stream_id: u32) -> Self {
        Self {
            frame,
            stream_id,
            url_origin: None,
            closed: Arc::new(AtomicBool::new(false)),
        }
    }

    /// A responder bound to a stream's document and lifetime.
    pub(crate) fn bound(
        frame: Frame,
        stream_id: u32,
        url_origin: Origin,
        closed: Arc<AtomicBool>,
    ) -> Self {
        Self {
            frame,
            stream_id,
            url_origin: Some(url_origin),
            closed,
        }
    }

    /// Whether anything may still be sent to the stream's document.
    fn deliverable(&self) -> Result<(), String> {
        if self.frame.is_valid() == 0 {
            return Err("frame destroyed".into());
        }
        if self.closed.load(Ordering::SeqCst) {
            return Err("stream closed".into());
        }
        if let Some(url_origin) = &self.url_origin {
            let url: CefString = (&self.frame.url()).into();
            if !still_addressed(url_origin, &url.to_string()) {
                return Err("the frame shows another document".into());
            }
        }
        Ok(())
    }

    /// Send a data chunk to the renderer.
    pub fn send_data(&self, data: &[u8]) -> Result<(), String> {
        self.deliverable()?;
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
        self.deliverable()?;
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
        self.deliverable()?;
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

/// Handles the lifecycle of a stream.
///
/// A handler is created when the stream opens and dropped when it ends or
/// errors. The framework supplies a responder to callbacks that may send
/// data back to the renderer.
///
/// on_chunk borrows the responder (the stream continues).
/// on_end takes ownership (the stream is consumed).
pub trait StreamHandler: Send + 'static {
    /// Called when the stream opens.
    fn on_open(&mut self, metadata: &str, responder: &StreamResponder) -> Result<(), String> {
        let _ = (metadata, responder);
        Ok(())
    }

    /// Called for each data chunk.
    fn on_chunk(&mut self, data: &[u8], responder: &StreamResponder) -> Result<(), String>;

    /// Called when the stream closes normally.
    fn on_end(&mut self, result: &str, responder: StreamResponder) -> Result<(), String> {
        let _ = (result, responder);
        Ok(())
    }

    /// Called when the stream errors.
    fn on_error(&mut self, message: &str) {
        let _ = message;
    }
}

/// Factory type: creates a new handler instance per stream.
pub type StreamFactory = Box<dyn Fn() -> Box<dyn StreamHandler> + Send + Sync>;

pub mod browser;
pub mod renderer;

/// An open stream and the document that owns it.
pub(crate) struct StreamEntry {
    browser_id: BrowserId,
    handler: Box<dyn StreamHandler>,
    frame: Frame,
    url_origin: Origin,
    closed: Arc<AtomicBool>,
}

impl StreamEntry {
    /// Returns a responder for this stream.
    fn responder(&self, stream_id: u32) -> StreamResponder {
        StreamResponder::bound(
            self.frame.clone(),
            stream_id,
            self.url_origin.clone(),
            self.closed.clone(),
        )
    }

    /// Prevents further responses on this stream.
    fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
    }
}

/// Identifies a stream by its opening frame, origin and id.
///
/// Only the opening frame, while showing the same origin, may use the stream.
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
    pub(crate) streams: Mutex<HashMap<StreamKey, StreamEntry>>,
}

impl StreamSubsystem {
    pub fn new(factories: HashMap<String, StreamFactory>) -> Self {
        Self {
            factories,
            streams: Mutex::new(HashMap::new()),
        }
    }

    /// Removes and closes the streams `remove` selects; returns how many.
    fn close_where(&self, mut remove: impl FnMut(&StreamKey, &StreamEntry) -> bool) -> usize {
        let mut streams = self.streams.lock().unwrap();
        let before = streams.len();
        streams.retain(|key, entry| {
            let gone = remove(key, entry);
            if gone {
                entry.close();
            }
            !gone
        });
        before - streams.len()
    }

    /// Remove the streams of `frame`, which is starting a new document.
    pub fn clear_frame(&self, frame: &FrameId) -> usize {
        self.close_where(|key, _| key.frame == *frame)
    }

    /// Remove all streams whose frame is no longer valid.
    pub fn clear_invalid_frames(&self) -> usize {
        self.close_where(|_, entry| entry.frame.is_valid() == 0)
    }

    /// Remove all streams for a given browser.
    pub fn clear_for_browser(&self, browser_id: BrowserId) -> usize {
        self.close_where(|_, entry| entry.browser_id == browser_id)
    }
}
