use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::debug;
use crate::ipc::browser_state::IpcError;

type Callback<T> = Box<dyn FnOnce(Result<T, IpcError>) + Send>;

/// Single-use callback for async request/response IPC.
///
/// If dropped without calling [`resolve`], the promise is automatically
/// rejected ensuring every pending request eventually settles.
pub struct Responder<T> {
    callback: Mutex<Option<Callback<T>>>,
    cancelled: Arc<AtomicBool>,
}

impl<T: 'static> Responder<T> {
    pub fn new(callback: Callback<T>) -> Self {
        Self {
            callback: Mutex::new(Some(callback)),
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Create a responder that shares a cancellation flag with a
    /// [`PendingEntry`](crate::ipc::pending::PendingEntry).
    pub fn with_abort(callback: Callback<T>, cancelled: Arc<AtomicBool>) -> Self {
        Self {
            callback: Mutex::new(Some(callback)),
            cancelled,
        }
    }

    /// Whether this responder has been cancelled.
    /// Typically by an incoming RPC_CANCEL from the renderer.
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    pub fn resolve(&self, result: Result<T, IpcError>) {
        let cb = self.callback.lock().unwrap().take();
        if let Some(cb) = cb {
            if self.cancelled.load(Ordering::SeqCst) {
                debug!("[IPC] dropping response for canceled responder");
                return;
            }
            cb(result);
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
    pub fn map<U, F>(self, f: F) -> Responder<U>
    where
        U: 'static,
        F: FnOnce(U) -> Result<T, IpcError> + Send + 'static,
    {
        let inner = self
            .callback
            .lock()
            .unwrap()
            .take()
            .expect("responder already resolved");
        let cancelled = self.cancelled.clone();
        let f = Mutex::new(Some(f));
        Responder {
            callback: Mutex::new(Some(Box::new(move |result: Result<U, IpcError>| {
                let mapped = result.and_then(|v| {
                    f.lock()
                        .unwrap()
                        .take()
                        .expect("responder map called twice")(v)
                });
                inner(mapped);
            }))),
            cancelled,
        }
    }
}

impl<T> Drop for Responder<T> {
    fn drop(&mut self) {
        if let Some(cb) = self.callback.lock().unwrap().take() {
            cb(Err(IpcError::new(
                "handler dropped responder without resolving",
                IpcError::CODE_DROPPED,
            )));
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    type CallRecord = (Result<i32, IpcError>,);
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
            res.lock().unwrap().push((result,));
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
        assert_eq!(r[0].0, Ok(42));
    }

    // Errors and error codes are forwarded unchanged to the callback
    #[test]
    fn resolve_with_error_forwards_code() {
        let (responder, call_count, results) = recording_responder();
        responder.resolve(Err(IpcError::new("something failed", -42)));
        assert_eq!(call_count.load(Ordering::SeqCst), 1);
        let r = results.lock().unwrap();
        assert_eq!(r[0].0.as_ref().unwrap_err().code(), -42);
    }

    // Once resolved, subsequent resolve calls are ignored
    #[test]
    fn resolve_twice_is_noop() {
        let (responder, call_count, results) = recording_responder();
        responder.resolve(Ok(1));
        responder.resolve(Ok(2)); // no-op
        assert_eq!(call_count.load(Ordering::SeqCst), 1);
        let r = results.lock().unwrap();
        assert_eq!(r[0].0, Ok(1));
    }

    // The first resolution wins regardless of success or failure
    #[test]
    fn resolve_error_then_ok_is_noop() {
        let (responder, call_count, _) = recording_responder();
        responder.resolve(Err(IpcError::new("first", -1)));
        responder.resolve(Ok(999));
        assert_eq!(call_count.load(Ordering::SeqCst), 1);
    }

    // Dropping an unresolved responder automatically rejects the request
    #[test]
    fn drop_without_resolve_auto_rejects() {
        let (responder, call_count, results) = recording_responder();
        drop(responder);
        assert_eq!(call_count.load(Ordering::SeqCst), 1);
        let r = results.lock().unwrap();
        let err = r[0].0.as_ref().unwrap_err();
        assert!(err.message().contains("dropped"));
        assert_eq!(err.code(), IpcError::CODE_DROPPED);
    }

    #[test]
    fn drop_after_resolve_does_not_call_again() {
        let (responder, call_count, _) = recording_responder();
        responder.resolve(Ok(10));
        drop(responder);
        assert_eq!(call_count.load(Ordering::SeqCst), 1);
    }

    // Automatic rejection identifies the responder as having been dropped
    #[test]
    fn drop_error_message_contains_dropped_text() {
        let results: RecordingResults = Arc::new(Mutex::new(Vec::new()));
        let res = results.clone();
        {
            let _responder: Responder<i32> = Responder::new(Box::new(move |result| {
                res.lock().unwrap().push((result,));
            }));
            // _responder dropped here
        }
        let r = results.lock().unwrap();
        assert!(r[0].0.as_ref().unwrap_err().message().contains("handler dropped responder without resolving"));
    }

    // Concurrent resolution invokes the callback at most once
    #[test]
    fn concurrent_resolve_is_safe() {
        use std::thread;

        let (responder, call_count, results) = recording_responder();
        let responder = Arc::new(responder);
        let mut handles = vec![];

        for i in 0..10 {
            let r = responder.clone();
            handles.push(thread::spawn(move || {
                r.resolve(Ok(i));
            }));
        }

        for h in handles {
            h.join().unwrap();
        }

        assert_eq!(call_count.load(Ordering::SeqCst), 1);
        let r = results.lock().unwrap();
        assert_eq!(r.len(), 1);
        // Result should be one of the Ok(i) values, not corrupted
        assert!(r[0].0.is_ok());
    }

    // Racing resolve against drop still invokes the callback exactly once
    #[test]
    fn concurrent_resolve_and_drop_is_safe() {
        use std::thread;

        let call_count = Arc::new(AtomicUsize::new(0));
        let cc = call_count.clone();

        let responder = Arc::new(Responder::<i32>::new(Box::new(move |_| {
            cc.fetch_add(1, Ordering::SeqCst);
        })));

        let r1 = responder.clone();
        let h1 = thread::spawn(move || {
            r1.resolve(Ok(1));
        });

        let r2 = responder.clone();
        let h2 = thread::spawn(move || {
            drop(r2);
        });

        h1.join().unwrap();
        h2.join().unwrap();

        assert_eq!(call_count.load(Ordering::SeqCst), 1);
    }

    // Cancellation prevents a resolved value from reaching the callback
    #[test]
    fn cancelled_responder_drops_result() {
        let call_count = Arc::new(AtomicUsize::new(0));
        let results: Arc<Mutex<Vec<CallRecord>>> = Arc::new(Mutex::new(Vec::new()));
        let cc = call_count.clone();
        let res = results.clone();

        let flag = Arc::new(AtomicBool::new(true));
        let responder = Responder::with_abort(Box::new(move |result| {
            cc.fetch_add(1, Ordering::SeqCst);
            res.lock().unwrap().push((result,));
        }), flag);
        assert!(responder.is_cancelled());
        responder.resolve(Ok(99));
        assert_eq!(call_count.load(Ordering::SeqCst), 0);
        assert!(results.lock().unwrap().is_empty());
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

        let responder: Responder<i32> =
            responder.map(|v: i32| Ok(serde_json::to_vec(&v).unwrap()));

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

        let responder: Responder<i32> = responder.map(|_v: i32| {
            Err(IpcError::new("mapping failed", -10))
        });

        responder.resolve(Ok(42));

        let r = results.lock().unwrap();
        assert_eq!(r[0].as_ref().unwrap_err().code(), -10);
    }
}
