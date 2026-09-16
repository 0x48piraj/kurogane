//! Renderer IPC state, shared across the renderer's V8 contexts.
//!
//! Access is synchronized with a short-lived lock that is never held while
//! calling into JavaScript, since IPC callbacks may re-enter. A poisoned lock
//! is recovered so a callback does not panic across the CEF boundary.

use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};

use cef::*;

use crate::ipc::renderer_registry::{ContextHandle, Registry};

pub(crate) type RendererState = Registry<V8Context, V8Value>;

impl ContextHandle for V8Context {
    fn same(&self, other: &Self) -> bool {
        let mut other = other.clone();
        self.is_same(Some(&mut other)) != 0
    }
}

static STATE: OnceLock<Mutex<RendererState>> = OnceLock::new();

/// The renderer's IPC state.
pub(crate) fn state() -> MutexGuard<'static, RendererState> {
    STATE
        .get_or_init(|| Mutex::new(Registry::new()))
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}
