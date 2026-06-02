//! Renderer process IPC state
//!
//! Manages promise registry and frame tracking.

use cef::*;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use crate::ipc::protocol::IpcId;
use crate::ipc::transport::shm::SharedBuffer;

//
// Promise registry: Tracks pending promises awaiting responses from the browser process
//

pub struct PromiseRegistry {
    next_id: IpcId,
    pending: HashMap<IpcId, (V8Context, V8Value)>,
}

impl Default for PromiseRegistry {
    fn default() -> Self {
        Self {
            next_id: 1,
            pending: HashMap::new(),
        }
    }
}

impl PromiseRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, context: V8Context, promise: V8Value) -> IpcId {
        let id = self.next_id;
        self.next_id = self.next_id.checked_add(1).unwrap_or(1);

        self.pending.insert(id, (context, promise));

        id
    }

    pub fn take(&mut self, id: IpcId) -> Option<(V8Context, V8Value)> {
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

static OUTGOING_SHM: OnceLock<Mutex<HashMap<IpcId, SharedBuffer>>> = OnceLock::new();

// ACCESSORS

pub fn registry() -> &'static Mutex<PromiseRegistry> {
    PROMISE_REGISTRY.get_or_init(Default::default)
}

pub fn outgoing_shm() -> &'static Mutex<HashMap<IpcId, SharedBuffer>> {
    OUTGOING_SHM.get_or_init(|| Mutex::new(HashMap::new()))
}

// HELPERS

pub fn register_promise(ctx: V8Context, promise: V8Value) -> IpcId {
    registry().lock().unwrap().register(ctx, promise)
}

pub fn clear_context_promises(ctx: &V8Context) {
    registry().lock().unwrap().clear_context(ctx);
}
