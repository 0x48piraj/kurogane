use std::sync::Mutex;

/// Single-use callback for async request/response IPC.
pub struct Responder<T> {
    callback: Mutex<Option<Box<dyn FnOnce(Result<T, String>, i32) + Send>>>,
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
