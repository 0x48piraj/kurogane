//! Renderer process IPC state
//!
//! Manages promise registry and frame tracking.

use cef::*;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use crate::ipc::transport::shm::SharedBuffer;

//
// Promise registry: Tracks pending promises awaiting responses from the browser process
//

pub struct PromiseRegistry {
    next_id: u32,
    pending: HashMap<u32, (V8Context, V8Value)>,
}

impl PromiseRegistry {
    pub fn new() -> Self {
        Self {
            next_id: 1,
            pending: HashMap::new(),
        }
    }

    pub fn register(&mut self, context: V8Context, promise: V8Value) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);

        self.pending.insert(id, (context, promise));
        id
    }

    pub fn take(&mut self, id: u32) -> Option<(V8Context, V8Value)> {
        self.pending.remove(&id)
    }

    pub fn clear_context(&mut self, ctx: &V8Context) {
        let mut target = ctx.clone();
        self.pending.retain(|_, (stored_ctx, _)| {
            stored_ctx.is_same(Some(&mut target)) == 0
        });
    }
}

// GLOBALS

static PROMISE_REGISTRY: OnceLock<Mutex<PromiseRegistry>> = OnceLock::new();

//
// SHM store for renderer->browser outgoing requests.
// Keeps the SHM alive until the browser's response arrives,
// proving the browser has already read the data.
//

static OUTGOING_SHM: OnceLock<Mutex<HashMap<u32, SharedBuffer>>> = OnceLock::new();

static RENDERER_FRAME: OnceLock<Mutex<Option<Frame>>> = OnceLock::new();

// ACCESSORS

pub fn registry() -> &'static Mutex<PromiseRegistry> {
    PROMISE_REGISTRY.get_or_init(|| Mutex::new(PromiseRegistry::new()))
}

pub fn outgoing_shm() -> &'static Mutex<HashMap<u32, SharedBuffer>> {
    OUTGOING_SHM.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn renderer_frame() -> &'static Mutex<Option<Frame>> {
    RENDERER_FRAME.get_or_init(|| Mutex::new(None))
}

// HELPERS

pub fn register_promise(ctx: V8Context, promise: V8Value) -> u32 {
    registry().lock().unwrap().register(ctx, promise)
}

pub fn clear_context_promises(ctx: &V8Context) {
    registry().lock().unwrap().clear_context(ctx);
}

#[inline(always)]
pub fn get_frame() -> Option<Frame> {
    renderer_frame().lock().unwrap().clone()
}
