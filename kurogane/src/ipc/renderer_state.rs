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

    /// Look up the event name for a subscription id.
    pub fn get_event_name(&self, id: i64) -> Option<String> {
        for (event_name, entries) in &self.callbacks {
            if entries.iter().any(|(sid, _, _)| *sid == id) {
                return Some(event_name.clone());
            }
        }
        None
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

/// Stream callback registry: Tracks data/end/error callbacks registered via core.onStreamData/End/Error.
#[derive(Default)]
pub struct StreamCallbackRegistry {
    data_callbacks: HashMap<i32, (V8Context, V8Value)>,
    end_callbacks: HashMap<i32, (V8Context, V8Value)>,
    error_callbacks: HashMap<i32, (V8Context, V8Value)>,
}

impl StreamCallbackRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_data(&mut self, stream_id: i32, ctx: V8Context, callback: V8Value) {
        self.data_callbacks.insert(stream_id, (ctx, callback));
    }

    pub fn register_end(&mut self, stream_id: i32, ctx: V8Context, callback: V8Value) {
        self.end_callbacks.insert(stream_id, (ctx, callback));
    }

    pub fn register_error(&mut self, stream_id: i32, ctx: V8Context, callback: V8Value) {
        self.error_callbacks.insert(stream_id, (ctx, callback));
    }

    /// Clone the data callback for invocation (persistent; does not remove).
    pub fn collect_data(&self, stream_id: i32) -> Option<(V8Context, V8Value)> {
        self.data_callbacks.get(&stream_id).map(|(ctx, cb)| (ctx.clone(), cb.clone()))
    }

    /// Remove and return the end callback (one-shot).
    pub fn take_end(&mut self, stream_id: i32) -> Option<(V8Context, V8Value)> {
        self.end_callbacks.remove(&stream_id)
    }

    /// Remove and return the error callback (one-shot).
    pub fn take_error(&mut self, stream_id: i32) -> Option<(V8Context, V8Value)> {
        self.error_callbacks.remove(&stream_id)
    }

    /// Remove all callbacks for a stream.
    pub fn clear_stream(&mut self, stream_id: i32) {
        self.data_callbacks.remove(&stream_id);
        self.end_callbacks.remove(&stream_id);
        self.error_callbacks.remove(&stream_id);
    }

    /// Remove all callbacks for a given V8 context.
    pub fn clear_context(&mut self, ctx: &V8Context) {
        let mut target = ctx.clone();
        self.data_callbacks.retain(|_, (stored_ctx, _)| {
            stored_ctx.is_same(Some(&mut target)) == 0
        });
        self.end_callbacks.retain(|_, (stored_ctx, _)| {
            stored_ctx.is_same(Some(&mut target)) == 0
        });
        self.error_callbacks.retain(|_, (stored_ctx, _)| {
            stored_ctx.is_same(Some(&mut target)) == 0
        });
    }
}

static STREAM_CALLBACK_REGISTRY: OnceLock<Mutex<StreamCallbackRegistry>> = OnceLock::new();

pub fn stream_callback_registry() -> &'static Mutex<StreamCallbackRegistry> {
    STREAM_CALLBACK_REGISTRY.get_or_init(Default::default)
}

pub fn clear_context_streams(ctx: &V8Context) {
    stream_callback_registry().lock().unwrap().clear_context(ctx);
}

pub type IpcId = i32;

/// Promise registry: Tracks pending promises awaiting responses from the browser process
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
        let start = self.next_id;
        loop {
            let id = self.next_id;
            self.next_id = self.next_id.checked_add(1).unwrap_or(1);
            if !self.pending.contains_key(&id) {
                self.pending.insert(id, (context, promise, subsystem));
                return id;
            }
            if self.next_id == start {
                panic!("PromiseRegistry exhausted all {} ids", IpcId::MAX as i64 + 1);
            }
        }
    }

    pub fn take(&mut self, id: IpcId) -> Option<(V8Context, V8Value, u8)> {
        self.pending.remove(&id)
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
