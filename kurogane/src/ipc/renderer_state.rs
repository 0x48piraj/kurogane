//! Renderer IPC state on the main thread.
//!
//! The main thread owns every V8 context and receives every IPC message.
//! No borrow spans a call into JavaScript; IPC callbacks may re-enter.

use std::cell::RefCell;

use tanso::*;

use crate::ipc::renderer_registry::{ContextHandle, Registry};

pub(crate) type RendererState = Registry<V8Context, V8Value>;

impl ContextHandle for V8Context {
    fn same(&self, other: &Self) -> bool {
        let mut other = other.clone();
        self.is_same(Some(&mut other)) != 0
    }
}

thread_local! {
    static STATE: RefCell<RendererState> = RefCell::new(Registry::new());
}

/// Runs `f` on the renderer's IPC state.
pub(crate) fn with_state<R>(f: impl FnOnce(&mut RendererState) -> R) -> R {
    STATE.with(|state| f(&mut state.borrow_mut()))
}
