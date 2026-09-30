//! Stream IPC subsystem.
//!
//! Provides bidirectional streaming data transport. Streams are identified
//! by a correlation ID. The page opens a stream; the browser answers with
//! `STREAM_BROWSER_OPENED` or refuses with `STREAM_BROWSER_ERROR`, and data
//! then flows both ways until the stream ends or fails.
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
use crate::runtime::AppHandle;

/// Responder for sending data from a browser-side stream handler to the renderer.
///
/// A responder the runtime hands a handler is bound to its stream and its
/// document. Once any clone has sent the stream's end or error, or the
/// stream is closed or its document replaced, every send returns `Err`.
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

    /// Sends a data chunk to the page's `onData`.
    ///
    /// # Errors
    ///
    /// Returns `Err` and sends nothing when the stream has ended or failed,
    /// its frame is gone or shows another document, or the message cannot
    /// be built.
    pub fn send_data(&self, data: &[u8]) -> Result<(), String> {
        self.deliverable()?;
        let mut msg = self.message(STREAM_BROWSER_DATA, PAYLOAD_BINARY, data)?;
        self.frame
            .send_process_message(ProcessId::RENDERER, Some(&mut msg));
        Ok(())
    }

    /// Ends the stream: the page's `onEnd` receives `result`.
    ///
    /// The first `end` or `error` sent, from any clone, closes the stream.
    ///
    /// # Errors
    ///
    /// As [`send_data`](Self::send_data); `Err("stream closed")` once the
    /// stream has ended or failed.
    pub fn end(&self, result: &str) -> Result<(), String> {
        self.finish(STREAM_BROWSER_END, PAYLOAD_STRING, result.as_bytes())
    }

    /// Fails the stream: the page's `onError` receives `msg`.
    ///
    /// Closes the stream, as [`end`](Self::end) does.
    ///
    /// # Errors
    ///
    /// As [`end`](Self::end).
    pub fn error(&self, msg: &str) -> Result<(), String> {
        self.error_with_code(msg, ErrorCode::Handler)
    }

    /// Fails the stream with class `code`: a pending open rejects with that code.
    pub(crate) fn error_with_code(&self, msg: &str, code: ErrorCode) -> Result<(), String> {
        let payload = encode_error_payload(code.wire(), msg);
        self.finish(STREAM_BROWSER_ERROR, PAYLOAD_BINARY, &payload)
    }

    /// Tells the page the handler accepted the open, which resolves its
    /// `openStream`. Sent before anything else of the stream.
    pub(crate) fn opened(&self) -> Result<(), String> {
        self.deliverable()?;
        let mut msg = self.message(STREAM_BROWSER_OPENED, PAYLOAD_EMPTY, &[])?;
        self.frame
            .send_process_message(ProcessId::RENDERER, Some(&mut msg));
        Ok(())
    }

    /// Sends the stream's last message. It is built first, so only a message
    /// that is sent closes the stream, and the swap lets exactly one end or
    /// error through, whichever thread sends it.
    fn finish(&self, opcode: u8, payload_kind: u8, payload: &[u8]) -> Result<(), String> {
        self.deliverable()?;
        let mut msg = self.message(opcode, payload_kind, payload)?;
        if self.closed.swap(true, Ordering::SeqCst) {
            return Err("stream closed".into());
        }
        self.frame
            .send_process_message(ProcessId::RENDERER, Some(&mut msg));
        Ok(())
    }

    /// Builds a message of this stream.
    fn message(
        &self,
        opcode: u8,
        payload_kind: u8,
        payload: &[u8],
    ) -> Result<ProcessMessage, String> {
        let envelope = Envelope {
            version: ENVELOPE_VERSION,
            subsystem: SUB_STREAM,
            opcode,
            flags: 0,
            correlation_id: self.stream_id,
            payload_kind,
        };
        build_message("kurogane_stream", &envelope, payload)
            .ok_or_else(|| format!("failed to build stream message {opcode}"))
    }
}

/// Handles one stream a page opens.
///
/// The factory registered with [`App::stream`](crate::App::stream) makes a
/// handler for each stream. In order:
///
/// 1. [`on_open`](Self::on_open) accepts or refuses the stream. Nothing can
///    be sent yet: the page does not hold the stream.
/// 2. The runtime tells the page the stream is open, which resolves its
///    `openStream`, then calls [`on_opened`](Self::on_opened).
/// 3. [`on_chunk`](Self::on_chunk) runs for each chunk the page writes.
/// 4. [`on_end`](Self::on_end) runs when the page ends the stream.
///
/// The first [`end`](StreamResponder::end) or
/// [`error`](StreamResponder::error) sent closes the stream, whichever clone
/// of the responder sends it; later sends return `Err("stream closed")`.
/// An `Err` from `on_open` rejects the open; an `Err` from `on_opened`,
/// `on_chunk` or `on_end` fails the stream unless it is already closed. A
/// panic in any of them counts as `Err("handler panicked")`. The page's end
/// is always answered: when `on_end` sends neither, the runtime ends the
/// stream with an empty result.
///
/// The runtime drops the handler once the stream is closed. A stream closed
/// from another thread is noticed at the next stream open, or when its
/// document goes.
///
/// # Threads
///
/// Every callback runs on the UI thread, which also runs every window and
/// receives every message from the page, so return promptly: CEF says not
/// to block this thread. The responder may be cloned and used from any
/// thread.
///
/// # Examples
///
/// A handler that greets the page, then counts the bytes the page writes:
///
/// ```no_run
/// use kurogane::App;
/// use kurogane::ipc::{StreamHandler, StreamResponder};
///
/// #[derive(Default)]
/// struct Counter {
///     bytes: usize,
/// }
///
/// impl StreamHandler for Counter {
///     fn on_opened(&mut self, responder: &StreamResponder) -> Result<(), String> {
///         responder.send_data(b"ready")
///     }
///
///     fn on_chunk(&mut self, data: &[u8], _: &StreamResponder) -> Result<(), String> {
///         self.bytes += data.len();
///         Ok(())
///     }
///
///     fn on_end(&mut self, _: &str, responder: StreamResponder) -> Result<(), String> {
///         responder.end(&self.bytes.to_string())
///     }
/// }
///
/// App::new("frontend").stream("count", Counter::default).run_or_exit();
/// ```
pub trait StreamHandler: Send + 'static {
    /// Accepts or refuses the stream. `metadata` is the second argument of
    /// the page's `openStream`. An `Err` rejects the `openStream` with its
    /// message.
    fn on_open(&mut self, metadata: &str) -> Result<(), String> {
        let _ = metadata;
        Ok(())
    }

    /// Called once the page has been told the stream is open: the place to
    /// send initial data, end or fail the stream, or hand a clone of
    /// `responder` to a thread that produces data. An `Err` fails the stream.
    fn on_opened(&mut self, responder: &StreamResponder) -> Result<(), String> {
        let _ = responder;
        Ok(())
    }

    /// Called for each chunk the page writes. An `Err` fails the stream.
    fn on_chunk(&mut self, data: &[u8], responder: &StreamResponder) -> Result<(), String>;

    /// Called when the page ends the stream with `result`. Once it returns
    /// `Ok`, the runtime ends the stream with an empty result unless the
    /// handler has ended or failed it; an `Err` fails the stream instead.
    fn on_end(&mut self, result: &str, responder: StreamResponder) -> Result<(), String> {
        let _ = (result, responder);
        Ok(())
    }

    /// Called when the renderer reports that the stream failed; not for a
    /// failure the handler or the runtime sends.
    fn on_error(&mut self, message: &str) {
        let _ = message;
    }
}

/// Factory type: creates a new handler instance per stream.
pub type StreamFactory = Box<dyn Fn(&AppHandle) -> Box<dyn StreamHandler> + Send + Sync>;

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

    /// Whether the stream has ended, failed or been closed.
    fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
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
