use std::sync::Mutex;

type Callback<T> = Box<dyn FnOnce(Result<T, String>, i32) + Send>;

/// Single-use callback for async request/response IPC.
///
/// If dropped without calling 'resolve', the promise is automatically
/// rejected ensuring every pending request eventually settles.
pub struct Responder<T> {
    callback: Mutex<Option<Callback<T>>>,
}

impl<T> Responder<T> {
    pub fn new(callback: Box<dyn FnOnce(Result<T, String>, i32) + Send>) -> Self {
        Self {
            callback: Mutex::new(Some(callback)),
        }
    }

    pub fn resolve(&self, result: Result<T, String>, error_code: i32) {
        let cb = self.callback.lock().unwrap().take();
        if let Some(cb) = cb {
            cb(result, error_code);
        }
    }
}

impl<T> Drop for Responder<T> {
    fn drop(&mut self) {
        if let Some(cb) = self.callback.lock().unwrap().take() {
            cb(Err("handler dropped responder without resolving".into()), -3);
        }
    }
}
