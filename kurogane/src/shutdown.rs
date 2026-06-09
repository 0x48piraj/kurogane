use std::sync::{Arc, atomic::{AtomicBool, Ordering}};

#[derive(Clone)]
pub struct ShutdownSignal {
    inner: Arc<AtomicBool>,
}

impl ShutdownSignal {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn request_shutdown(&self) {
        self.inner.store(true, Ordering::Release);
    }

    pub fn is_shutdown_requested(&self) -> bool {
        self.inner.load(Ordering::Acquire)
    }
}
