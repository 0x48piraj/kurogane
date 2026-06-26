//! Renderer process IPC state
//!
//! Manages promise registry and frame tracking.

use cef::*;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// Tracks V8 callbacks registered via core.on().
pub struct EventCallbackRegistry {
    next_id: i64,
    // Callbacks registered per event name
    callbacks: HashMap<String, Vec<(i64, V8Context, V8Value)>>,
}

impl Default for EventCallbackRegistry {
    fn default() -> Self {
        Self {
            next_id: 1,
            callbacks: HashMap::new(),
        }
    }
}

impl EventCallbackRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a callback for an event. Returns subscription id.
    pub fn register(&mut self, event: &str, ctx: V8Context, callback: V8Value) -> i64 {
        let id = self.next_id;
        self.next_id = self.next_id.checked_add(1).unwrap_or(1);
        self.callbacks
            .entry(event.to_string())
            .or_default()
            .push((id, ctx, callback));
        id
    }

    /// Unregister a callback by subscription id.
    pub fn unregister(&mut self, id: i64) -> bool {
        for (_, callbacks) in self.callbacks.iter_mut() {
            let before = callbacks.len();
            callbacks.retain(|(sid, _, _)| *sid != id);
            if callbacks.len() != before {
                return true;
            }
        }
        false
    }

    /// Unregister all callbacks for a specific event name.
    pub fn unregister_all(&mut self, event: &str) -> bool {
        self.callbacks.remove(event).is_some()
    }

    /// Collect callbacks for an event without invoking them.
    /// Lock must be released before calling into JS to avoid reentrant deadlock.
    pub fn collect_callbacks(&mut self, event: &str) -> Vec<(V8Context, V8Value)> {
        match self.callbacks.get(event) {
            Some(entries) => entries.iter().map(|(_, ctx, cb)| {
                (ctx.clone(), cb.clone())
            }).collect(),
            None => Vec::new(),
        }
    }

    /// Trigger all callbacks for an event. Returns number of callbacks triggered.
    pub fn trigger(&mut self, event: &str, payload: &str) -> usize {
        let Some(entries) = self.callbacks.get(event) else {
            return 0;
        };

        // Collect entries to call outside the mutable borrow
        let to_call: Vec<(i64, V8Context, V8Value)> = entries.iter().map(|(id, ctx, cb)| {
            (id.clone(), ctx.clone(), cb.clone())
        }).collect();
        let count = to_call.len();

        for (_id, context, callback) in to_call {
            if context.enter() == 0 {
                continue;
            }
            let payload_v8 = v8_value_create_string(Some(&CefString::from(payload))).unwrap();
            let args: [Option<V8Value>; 1] = [Some(payload_v8)];
            callback.execute_function(
                None,
                Some(&args),
            );
            context.exit();
        }

        count
    }

    pub fn clear_context(&mut self, ctx: &V8Context) {
        let mut target = ctx.clone();
        self.callbacks.retain(|_, callbacks| {
            callbacks.retain(|(_, stored_ctx, _)| {
                stored_ctx.is_same(Some(&mut target)) == 0
            });
            !callbacks.is_empty()
        });
    }
}

static EVENT_REGISTRY: OnceLock<Mutex<EventCallbackRegistry>> = OnceLock::new();

pub fn event_registry() -> &'static Mutex<EventCallbackRegistry> {
    EVENT_REGISTRY.get_or_init(Default::default)
}

pub fn clear_context_events(ctx: &V8Context) {
    event_registry().lock().unwrap().clear_context(ctx);
}

//
// Promise registry: Tracks pending promises awaiting responses from the browser process
//

pub type IpcId = i32;

pub struct PromiseRegistry {
    next_id: IpcId,
    pending: HashMap<IpcId, (V8Context, V8Value, u8)>,
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

    pub fn register(&mut self, context: V8Context, promise: V8Value, subsystem: u8) -> IpcId {
        let id = self.next_id;
        self.next_id = self.next_id.checked_add(1).unwrap_or(1);

        self.pending.insert(id, (context, promise, subsystem));

        id
    }

    pub fn take(&mut self, id: IpcId) -> Option<(V8Context, V8Value, u8)> {
        self.pending.remove(&id).map(|(ctx, promise, sub)| (ctx, promise, sub))
    }

    pub fn clear_context(&mut self, ctx: &V8Context) {
        let mut target = ctx.clone();
        self.pending.retain(|_, (stored_ctx, _, _)| {
            stored_ctx.is_same(Some(&mut target)) == 0
        });
    }
}

// GLOBALS

static PROMISE_REGISTRY: OnceLock<Mutex<PromiseRegistry>> = OnceLock::new();

// ACCESSORS

pub fn registry() -> &'static Mutex<PromiseRegistry> {
    PROMISE_REGISTRY.get_or_init(Default::default)
}

// HELPERS

pub fn register_promise(ctx: V8Context, promise: V8Value, subsystem: u8) -> IpcId {
    registry().lock().unwrap().register(ctx, promise, subsystem)
}

pub fn cancel_promise(id: IpcId) -> Option<(V8Context, V8Value, u8)> {
    registry().lock().unwrap().take(id)
}

pub fn clear_context_promises(ctx: &V8Context) {
    registry().lock().unwrap().clear_context(ctx);
}
