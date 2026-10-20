//! Stream IPC subsystem.
//!
//! Provides bidirectional streaming data transport. Streams are identified
//! by a correlation ID. The page opens a stream. The browser answers with
//! `STREAM_BROWSER_OPENED` or refuses with `STREAM_BROWSER_ERROR`. Data then
//! flows both ways until the stream ends or fails.
//!
//! Each stream gets its own handler instance via a factory closure,
//! giving handlers natural per-stream mutable state.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tanso::*;

use crate::ipc::browser_outbox;
use crate::ipc::document::{Document, DocumentKey};
use crate::ipc::envelope::*;
use crate::ipc::{FrameId, IpcContext, IpcError};
use crate::runtime::AppHandle;

/// Responder for sending data from a browser-side stream handler to the renderer.
///
/// A responder the runtime hands a handler is bound to its stream and its
/// document. Once any clone has sent the stream's end or error, or the
/// stream is closed or its document has ended, every send returns `Err`.
#[derive(Clone)]
pub struct StreamResponder {
    frame: Frame,
    frame_id: FrameId,
    stream_id: u32,
    /// Document that opened the stream. Responses are sent only while it
    /// lives.
    document: Option<Arc<Document>>,
    /// Shared stream state.
    closed: Arc<AtomicBool>,
}

impl StreamResponder {
    /// Returns a responder for an open the runtime refuses as it arrives,
    /// before the stream exists. The page waiting on `openStream` is the
    /// document that sent the open and there is nothing to check. Never
    /// handed to a handler.
    pub(crate) fn unbound(frame: Frame, stream_id: u32) -> Self {
        Self {
            frame_id: FrameId::of(&frame),
            frame,
            stream_id,
            document: None,
            closed: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Returns a responder bound to a stream's document and lifetime.
    pub(crate) fn bound(
        frame: Frame,
        document: Arc<Document>,
        stream_id: u32,
        closed: Arc<AtomicBool>,
    ) -> Self {
        Self {
            frame,
            frame_id: document.frame().clone(),
            stream_id,
            document: Some(document),
            closed,
        }
    }

    /// Whether anything may still be sent to the stream's document.
    fn deliverable(&self) -> Result<(), IpcError> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(IpcError::new("stream closed"));
        }
        if self
            .document
            .as_ref()
            .is_some_and(|document| !document.is_live())
        {
            return Err(IpcError::new("the document has ended"));
        }
        Ok(())
    }

    /// Sends a data chunk to the page's `onData`.
    ///
    /// # Errors
    ///
    /// Returns `Err` and sends nothing when the stream has ended or failed,
    /// or the document that opened it has gone (its frame navigated or
    /// closed).
    pub fn send_data(&self, data: &[u8]) -> Result<(), IpcError> {
        self.deliverable()?;
        self.send(STREAM_BROWSER_DATA, PAYLOAD_BINARY, data);
        Ok(())
    }

    /// Ends the stream. The page's `onEnd` receives `result`.
    ///
    /// The first `end` or `error` sent, from any clone, closes the stream.
    ///
    /// # Errors
    ///
    /// As [`send_data`](Self::send_data); the error's message is `stream
    /// closed` once the stream has ended or failed.
    pub fn end(&self, result: &str) -> Result<(), IpcError> {
        self.finish(STREAM_BROWSER_END, PAYLOAD_STRING, result.as_bytes())
    }

    /// Fails the stream. The page's `onError` receives the error's message.
    /// A string is an [`IpcError::new`].
    ///
    /// Closes the stream, as [`end`](Self::end) does.
    ///
    /// # Errors
    ///
    /// As [`end`](Self::end).
    pub fn error(&self, error: impl Into<IpcError>) -> Result<(), IpcError> {
        let error = error.into();
        let payload = encode_error_payload(error.code().wire(), error.message());
        self.finish(STREAM_BROWSER_ERROR, PAYLOAD_BINARY, &payload)
    }

    /// Tells the page the handler accepted the open, which resolves its
    /// `openStream`. Sent before anything else of the stream.
    pub(crate) fn opened(&self) -> Result<(), IpcError> {
        self.deliverable()?;
        self.send(STREAM_BROWSER_OPENED, PAYLOAD_EMPTY, &[]);
        Ok(())
    }

    /// Sends the stream's last message. The swap lets exactly one end or
    /// error through, whichever thread sends it.
    fn finish(&self, opcode: u8, payload_kind: u8, payload: &[u8]) -> Result<(), IpcError> {
        self.deliverable()?;
        if self.closed.swap(true, Ordering::SeqCst) {
            return Err(IpcError::new("stream closed"));
        }
        self.send(opcode, payload_kind, payload);
        Ok(())
    }

    /// Sends a message of this stream.
    fn send(&self, opcode: u8, payload_kind: u8, payload: &[u8]) {
        let envelope = Envelope {
            version: ENVELOPE_VERSION,
            subsystem: SUB_STREAM,
            opcode,
            flags: 0,
            correlation_id: self.stream_id,
            payload_kind,
        };
        browser_outbox::send(&self.frame, &self.frame_id, &envelope, payload);
    }
}

/// Handles one stream a page opens.
///
/// The factory registered with [`App::stream`](crate::App::stream) makes a
/// handler for each stream. In order:
///
/// 1. [`on_open`](Self::on_open) accepts or refuses the stream. Nothing can
///    be sent yet since the page does not hold the stream.
/// 2. The runtime tells the page the stream is open, which resolves its
///    `openStream`, then calls [`on_opened`](Self::on_opened).
/// 3. [`on_chunk`](Self::on_chunk) runs for each chunk the page writes.
/// 4. [`on_end`](Self::on_end) runs when the page ends the stream.
///
/// The first [`end`](StreamResponder::end) or
/// [`error`](StreamResponder::error) sent closes the stream, whichever clone
/// of the responder sends it; later sends fail with `stream closed`.
/// An `Err` from `on_open` rejects the open with the error's message and
/// code, as a command's error does; an `Err` from `on_opened`, `on_chunk` or
/// `on_end` fails the stream, whose `onError` receives the message, unless it
/// is already closed. A panic in any of them counts as an `Err` with the
/// message `handler panicked`. The page's end is always answered. When
/// `on_end` sends neither the runtime ends the stream with an empty result.
///
/// The runtime drops the handler once the stream is closed. A stream closed
/// from another thread is noticed at the next stream open, or when its
/// document goes.
///
/// # Threads
///
/// Every callback runs on the UI thread. That thread also runs every window
/// and receives every message from the page. Return promptly; CEF says not
/// to block this thread. The responder may be cloned and used from any
/// thread.
///
/// # Examples
///
/// A handler that greets the page, then counts the bytes the page writes:
///
/// ```no_run
/// use kurogane::{App, IpcError, StreamHandler, StreamResponder};
///
/// #[derive(Default)]
/// struct Counter {
///     bytes: usize,
/// }
///
/// impl StreamHandler for Counter {
///     fn on_opened(&mut self, responder: &StreamResponder) -> Result<(), IpcError> {
///         responder.send_data(b"ready")
///     }
///
///     fn on_chunk(&mut self, data: &[u8], _: &StreamResponder) -> Result<(), IpcError> {
///         self.bytes += data.len();
///         Ok(())
///     }
///
///     fn on_end(&mut self, _: &str, responder: StreamResponder) -> Result<(), IpcError> {
///         responder.end(&self.bytes.to_string())
///     }
/// }
///
/// App::new("frontend").stream("count", Counter::default).run_or_exit();
/// ```
pub trait StreamHandler: Send + 'static {
    /// Accepts or refuses the stream. `metadata` is the second argument of
    /// the page's `openStream`. An `Err` rejects the `openStream` with its
    /// message and code.
    fn on_open(&mut self, metadata: &str) -> Result<(), IpcError> {
        let _ = metadata;
        Ok(())
    }

    /// Called once the page has been told the stream is open. It is the place
    /// to send initial data, to end or fail the stream or to hand a clone of
    /// `responder` to a thread that produces data. An `Err` fails the stream.
    fn on_opened(&mut self, responder: &StreamResponder) -> Result<(), IpcError> {
        let _ = responder;
        Ok(())
    }

    /// Called for each chunk the page writes. An `Err` fails the stream.
    fn on_chunk(&mut self, data: &[u8], responder: &StreamResponder) -> Result<(), IpcError>;

    /// Called when the page ends the stream with `result`. Once it returns
    /// `Ok`, the runtime ends the stream with an empty result unless the
    /// handler has ended or failed it; an `Err` fails the stream instead.
    fn on_end(&mut self, result: &str, responder: StreamResponder) -> Result<(), IpcError> {
        let _ = (result, responder);
        Ok(())
    }

    /// Called when the renderer reports that the stream failed; not for a
    /// failure the handler or the runtime sends.
    fn on_error(&mut self, message: &str) {
        let _ = message;
    }
}

/// Factory that creates a new handler instance per stream.
pub type StreamFactory = Box<dyn Fn(&AppHandle) -> Box<dyn StreamHandler> + Send + Sync>;

pub mod browser;
pub mod renderer;

/// An open stream and the document that owns it.
pub(crate) struct StreamEntry {
    document: Arc<Document>,
    handler: Box<dyn StreamHandler>,
    frame: Frame,
    closed: Arc<AtomicBool>,
}

impl StreamEntry {
    /// Returns a responder for the stream `key` names.
    fn responder(&self, key: &StreamKey) -> StreamResponder {
        StreamResponder::bound(
            self.frame.clone(),
            self.document.clone(),
            key.id,
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

/// Identifies a stream by its opening document and id.
///
/// Only the opening document may use the stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct StreamKey {
    document: DocumentKey,
    id: u32,
}

impl StreamKey {
    pub(crate) fn new(ctx: &IpcContext, id: u32) -> Self {
        Self {
            document: ctx.document.key(),
            id,
        }
    }
}

/// Browser-side stream manager.
pub struct StreamSubsystem {
    pub factories: HashMap<String, StreamFactory>,
    /// Per-stream handler instances, keyed by opening document and stream id.
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

    /// Removes and closes the streams of `document`, which has ended.
    pub(crate) fn close_document(&self, document: DocumentKey) -> usize {
        self.close_where(|key, _| key.document == document)
    }
}
