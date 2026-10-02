use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tracing::debug;
use crate::ipc::browser_state::{ErrorCode, IpcError};

type Callback<T> = Box<dyn FnOnce(Result<T, IpcError>) + Send>;

/// The answer to one async request.
///
/// Resolving it takes it, so a request is answered once. If dropped without
/// calling [`resolve`](Responder::resolve), the promise is automatically
/// rejected ensuring every pending request eventually settles.
pub struct Responder<T> {
    /// Taken by `resolve` or `map`; whatever `Drop` still finds, it rejects.
    callback: Option<Callback<T>>,
    cancelled: Arc<AtomicBool>,
}

impl<T: 'static> Responder<T> {
    /// A responder that hands its result to `callback`: what the runtime
    /// gives an async handler, and what a test of one can give it.
    pub fn new(callback: Callback<T>) -> Self {
        Self {
            callback: Some(callback),
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Creates a responder controlled by the shared `cancelled` flag.
    /// Once it is set, [`resolve`](Self::resolve) sends nothing.
    pub(crate) fn with_abort(callback: Callback<T>, cancelled: Arc<AtomicBool>) -> Self {
        Self {
            callback: Some(callback),
            cancelled,
        }
    }

    /// Whether this responder has been cancelled.
    /// Typically by an incoming RPC_CANCEL from the renderer.
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    /// Answers the request, unless it was cancelled meanwhile.
    pub fn resolve(mut self, result: Result<T, IpcError>) {
        if let Some(callback) = self.callback.take() {
            if self.is_cancelled() {
                debug!("[IPC] dropping response for canceled responder");
                return;
            }
            callback(result);
        }
    }

    /// Transform the resolved value type.
    ///
    /// The returned responder chains through `f` before calling the original
    /// callback. Useful for wrapping a typed `Responder<Res>` into a
    /// `Responder<Vec<u8>>` at the serialisation boundary.
    ///
    /// The mapped responder shares the cancellation flag with the source.
    /// Cancelling the source or the mapped responder cancels both.
    pub fn map<U, F>(mut self, f: F) -> Responder<U>
    where
        U: 'static,
        F: FnOnce(U) -> Result<T, IpcError> + Send + 'static,
    {
        let callback = self.callback.take().map(|inner| -> Callback<U> {
            Box::new(move |result: Result<U, IpcError>| inner(result.and_then(f)))
        });
        Responder {
            callback,
            cancelled: self.cancelled.clone(),
        }
    }
}

impl<T> Drop for Responder<T> {
    fn drop(&mut self) {
        // The requester is gone, so there is no response to deliver
        if let Some(cb) = self.callback.take()
            && !self.cancelled.load(Ordering::SeqCst)
        {
            cb(Err(IpcError::with_code(
                "handler dropped responder without resolving",
                ErrorCode::Dropped,
            )));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::sync::atomic::AtomicUsize;

    type CallRecord = Result<i32, IpcError>;
    type RecordingResults = Arc<Mutex<Vec<CallRecord>>>;
    type BinaryResults = Arc<Mutex<Vec<Result<Vec<u8>, IpcError>>>>;

    /// Creates a responder that records callback invocations for assertions
    fn recording_responder() -> (Responder<i32>, Arc<AtomicUsize>, RecordingResults) {
        let call_count = Arc::new(AtomicUsize::new(0));
        let results: RecordingResults = Arc::new(Mutex::new(Vec::new()));
        let cc = call_count.clone();
        let res = results.clone();
        let responder = Responder::new(Box::new(move |result| {
            cc.fetch_add(1, Ordering::SeqCst);
            res.lock().unwrap().push(result);
        }));
        (responder, call_count, results)
    }

    // Resolving a responder invokes its callback exactly once
    #[test]
    fn resolve_once_invokes_callback() {
        let (responder, call_count, results) = recording_responder();
        responder.resolve(Ok(42));
        assert_eq!(call_count.load(Ordering::SeqCst), 1);
        let r = results.lock().unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0], Ok(42));
    }

    // Errors and error codes are forwarded unchanged to the callback
    #[test]
    fn resolve_with_error_forwards_code() {
        let (responder, call_count, results) = recording_responder();
        let code = ErrorCode::App(std::num::NonZeroU16::new(42).unwrap());
        responder.resolve(Err(IpcError::with_code("something failed", code)));
        assert_eq!(call_count.load(Ordering::SeqCst), 1);
        let r = results.lock().unwrap();
        assert_eq!(r[0].as_ref().unwrap_err().code(), code);
    }

    // Dropping an unresolved responder automatically rejects the request
    #[test]
    fn drop_without_resolve_auto_rejects() {
        let (responder, call_count, results) = recording_responder();
        drop(responder);
        assert_eq!(call_count.load(Ordering::SeqCst), 1);
        let r = results.lock().unwrap();
        let err = r[0].as_ref().unwrap_err();
        assert!(err.message().contains("dropped"));
        assert_eq!(err.code(), ErrorCode::Dropped);
    }

    // A resolved responder is not rejected as it goes
    #[test]
    fn a_resolved_responder_is_not_rejected_when_it_drops() {
        let (responder, call_count, results) = recording_responder();
        responder.resolve(Ok(10));
        assert_eq!(call_count.load(Ordering::SeqCst), 1);
        assert_eq!(results.lock().unwrap()[0], Ok(10));
    }

    // Automatic rejection identifies the responder as having been dropped
    #[test]
    fn drop_error_message_contains_dropped_text() {
        let results: RecordingResults = Arc::new(Mutex::new(Vec::new()));
        let res = results.clone();
        {
            let _responder: Responder<i32> = Responder::new(Box::new(move |result| {
                res.lock().unwrap().push(result);
            }));
            // _responder dropped here
        }
        let r = results.lock().unwrap();
        assert!(
            r[0].as_ref()
                .unwrap_err()
                .message()
                .contains("handler dropped responder without resolving")
        );
    }

    // Cancellation prevents a resolved value from reaching the callback
    #[test]
    fn cancelled_responder_drops_result() {
        let call_count = Arc::new(AtomicUsize::new(0));
        let results: Arc<Mutex<Vec<CallRecord>>> = Arc::new(Mutex::new(Vec::new()));
        let cc = call_count.clone();
        let res = results.clone();

        let flag = Arc::new(AtomicBool::new(true));
        let responder = Responder::with_abort(
            Box::new(move |result| {
                cc.fetch_add(1, Ordering::SeqCst);
                res.lock().unwrap().push(result);
            }),
            flag,
        );
        assert!(responder.is_cancelled());
        responder.resolve(Ok(99));
        assert_eq!(call_count.load(Ordering::SeqCst), 0);
        assert!(results.lock().unwrap().is_empty());
    }

    // Dropping a cancelled responder sends nothing: no rejection reaches a
    // page that cancelled, or a document that replaced the requester
    #[test]
    fn dropping_a_cancelled_responder_sends_nothing() {
        let call_count = Arc::new(AtomicUsize::new(0));
        let cc = call_count.clone();
        let flag = Arc::new(AtomicBool::new(false));
        let responder: Responder<i32> = Responder::with_abort(
            Box::new(move |_| {
                cc.fetch_add(1, Ordering::SeqCst);
            }),
            flag.clone(),
        );
        flag.store(true, Ordering::SeqCst);
        drop(responder);
        assert_eq!(call_count.load(Ordering::SeqCst), 0);
    }

    // Responder and pending RPC share the same cancellation state
    #[test]
    fn with_abort_shares_cancellation_flag() {
        let flag = Arc::new(AtomicBool::new(false));
        let responder: Responder<i32> = Responder::with_abort(Box::new(|_| {}), flag.clone());
        assert!(!responder.is_cancelled());
        flag.store(true, Ordering::SeqCst);
        assert!(responder.is_cancelled());
    }

    // Mapping transforms a typed responder into a different response type
    #[test]
    fn map_transforms_value_type() {
        let results: BinaryResults = Arc::new(Mutex::new(Vec::new()));
        let res = results.clone();

        let responder: Responder<Vec<u8>> = Responder::new(Box::new(move |result| {
            res.lock().unwrap().push(result);
        }));

        let responder: Responder<i32> = responder.map(|v: i32| Ok(serde_json::to_vec(&v).unwrap()));

        responder.resolve(Ok(42));

        let r = results.lock().unwrap();
        assert_eq!(r[0].as_ref().unwrap(), b"42");
    }

    // Mapping propagates errors produced by the transformation
    #[test]
    fn map_propagates_error() {
        let results: BinaryResults = Arc::new(Mutex::new(Vec::new()));
        let res = results.clone();

        let responder: Responder<Vec<u8>> = Responder::new(Box::new(move |result| {
            res.lock().unwrap().push(result);
        }));

        let code = ErrorCode::App(std::num::NonZeroU16::new(10).unwrap());
        let responder: Responder<i32> =
            responder.map(move |_v: i32| Err(IpcError::with_code("mapping failed", code)));

        responder.resolve(Ok(42));

        let r = results.lock().unwrap();
        assert_eq!(r[0].as_ref().unwrap_err().code(), code);
    }

    // Mapping preserves the cancellation state of the original responder
    #[test]
    fn map_preserves_cancellation() {
        let call_count = Arc::new(AtomicUsize::new(0));
        let cc = call_count.clone();

        let flag = Arc::new(AtomicBool::new(false));

        let responder: Responder<i32> = Responder::with_abort(
            Box::new(move |_| {
                cc.fetch_add(1, Ordering::SeqCst);
            }),
            flag.clone(),
        );

        let responder: Responder<String> =
            responder.map(|v: String| v.parse::<i32>().map_err(|e| IpcError::new(e.to_string())));

        flag.store(true, Ordering::SeqCst);

        assert!(responder.is_cancelled());
        responder.resolve(Ok("42".to_string()));
        assert_eq!(call_count.load(Ordering::SeqCst), 0);
    }

    // A mapped responder observes cancellation changes to the shared flag
    #[test]
    fn map_observes_external_cancellation() {
        let flag = Arc::new(AtomicBool::new(false));
        let inner: Responder<i32> = Responder::with_abort(Box::new(|_| {}), flag.clone());

        let mapped: Responder<String> =
            inner.map(|v: String| v.parse::<i32>().map_err(|e| IpcError::new(e.to_string())));

        assert!(!mapped.is_cancelled());
        flag.store(true, Ordering::SeqCst);
        assert!(mapped.is_cancelled());
    }

    // A cancelled mapped responder skips transformation and callback delivery
    #[test]
    fn map_cancelled_responder_skips_transform() {
        let transform_calls = Arc::new(AtomicUsize::new(0));
        let callback_calls = Arc::new(AtomicUsize::new(0));

        let transforms = transform_calls.clone();
        let callbacks = callback_calls.clone();

        let flag = Arc::new(AtomicBool::new(false));

        let inner: Responder<Vec<u8>> = Responder::with_abort(
            Box::new(move |_| {
                callbacks.fetch_add(1, Ordering::SeqCst);
            }),
            flag.clone(),
        );

        let mapped: Responder<i32> = inner.map(move |value| {
            transforms.fetch_add(1, Ordering::SeqCst);
            Ok(serde_json::to_vec(&value).unwrap())
        });

        flag.store(true, Ordering::SeqCst);

        mapped.resolve(Ok(42));

        assert_eq!(transform_calls.load(Ordering::SeqCst), 0);
        assert_eq!(callback_calls.load(Ordering::SeqCst), 0);
    }
}
