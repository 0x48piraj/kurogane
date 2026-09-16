//! Renderer-side IPC state for V8 contexts.
//!
//! Each pending promise, stream and event subscription belongs to one
//! context in one frame. Messages are delivered only to the owning frame.
//! Cancellation and unsubscription are limited to the owning context.
//!
//! Releasing a context removes all state owned by it. Unknown contexts are
//! opaque and use `FLAG_OPAQUE_CONTEXT`.
//!
//! Stream callbacks are set when the stream is opened.
//!
//! Generic over the context handle so the rules can be tested without V8.
//! `renderer_state` supplies the V8 context and value types.

use std::collections::HashMap;

use crate::ipc::envelope::{FLAG_OPAQUE_CONTEXT, SUB_RPC, SUB_STREAM};
use crate::ipc::FrameId;

/// A V8 context, or a stand-in in tests.
pub(crate) trait ContextHandle: Clone {
    fn same(&self, other: &Self) -> bool;
}

/// The callbacks a page supplies when it opens a stream.
pub(crate) struct StreamSink<V> {
    pub(crate) data: V,
    pub(crate) end: V,
    pub(crate) error: V,
}

struct Owner<C> {
    context: C,
    frame: FrameId,
}

enum Pending<V> {
    Rpc(V),
    StreamOpen { promise: V, sink: StreamSink<V> },
}

struct Subscription<V> {
    event: String,
    callback: V,
    on_error: Option<V>,
}

struct Known<C> {
    context: C,
    frame: FrameId,
    opaque: bool,
}

pub(crate) struct Registry<C, V> {
    next_id: i32,
    contexts: Vec<Known<C>>,
    pending: HashMap<i32, (Owner<C>, Pending<V>)>,
    streams: HashMap<i32, (Owner<C>, StreamSink<V>)>,
    events: HashMap<i32, (Owner<C>, Subscription<V>)>,
}

impl<C: ContextHandle, V: Clone> Default for Registry<C, V> {
    fn default() -> Self {
        Self::new()
    }
}

impl<C: ContextHandle, V: Clone> Registry<C, V> {
    pub(crate) fn new() -> Self {
        Registry {
            next_id: random_start(),
            contexts: Vec::new(),
            pending: HashMap::new(),
            streams: HashMap::new(),
            events: HashMap::new(),
        }
    }

    /// Records a new context of `frame`; `opaque` when its document has an
    /// opaque origin (a sandboxed frame).
    pub(crate) fn context_created(&mut self, context: C, frame: FrameId, opaque: bool) {
        self.contexts.retain(|known| !known.context.same(&context));
        self.contexts.push(Known {
            context,
            frame,
            opaque,
        });
    }

    /// Forgets a released context and everything it owned.
    pub(crate) fn context_released(&mut self, context: &C) {
        self.contexts.retain(|known| !known.context.same(context));
        self.pending
            .retain(|_, (owner, _)| !owner.context.same(context));
        self.streams
            .retain(|_, (owner, _)| !owner.context.same(context));
        self.events
            .retain(|_, (owner, _)| !owner.context.same(context));
    }

    /// Returns the envelope flags for `context`, treating unknown contexts as opaque.
    pub(crate) fn flags_for(&self, context: &C) -> u8 {
        match self.known(context) {
            Some(known) if !known.opaque => 0,
            _ => FLAG_OPAQUE_CONTEXT,
        }
    }

    pub(crate) fn register_rpc(&mut self, context: &C, promise: V) -> Option<i32> {
        let owner = self.owner(context)?;
        let id = self.allocate();
        self.pending.insert(id, (owner, Pending::Rpc(promise)));
        Some(id)
    }

    pub(crate) fn register_stream_open(
        &mut self,
        context: &C,
        promise: V,
        sink: StreamSink<V>,
    ) -> Option<i32> {
        let owner = self.owner(context)?;
        let id = self.allocate();
        self.pending
            .insert(id, (owner, Pending::StreamOpen { promise, sink }));
        Some(id)
    }

    /// Drops a pending entry whose request was never sent.
    pub(crate) fn forget(&mut self, id: i32) {
        self.pending.remove(&id);
    }

    /// Cancels a pending request, only for the context that made it. Returns
    /// its context, promise and subsystem.
    pub(crate) fn cancel(&mut self, id: i32, current: &C) -> Option<(C, V, u8)> {
        let (owner, _) = self.pending.get(&id)?;
        if !owner.context.same(current) {
            return None;
        }
        let (owner, pending) = self.pending.remove(&id)?;
        Some(match pending {
            Pending::Rpc(promise) => (owner.context, promise, SUB_RPC),
            Pending::StreamOpen { promise, .. } => (owner.context, promise, SUB_STREAM),
        })
    }

    /// Takes the promise of an RPC answered to `addressed`.
    pub(crate) fn settle(&mut self, id: i32, addressed: &FrameId) -> Option<(C, V)> {
        match self.pending.get(&id) {
            Some((owner, Pending::Rpc(_))) if owner.frame == *addressed => {}
            _ => return None,
        }
        match self.pending.remove(&id)? {
            (owner, Pending::Rpc(promise)) => Some((owner.context, promise)),
            (_, Pending::StreamOpen { .. }) => None,
        }
    }

    /// A stream opened: its callbacks become live; returns the open promise.
    pub(crate) fn stream_opened(&mut self, id: i32, addressed: &FrameId) -> Option<(C, V)> {
        let (owner, promise, sink) = self.take_stream_open(id, addressed)?;
        let context = owner.context.clone();
        self.streams.insert(id, (owner, sink));
        Some((context, promise))
    }

    /// A stream failed to open; returns the open promise.
    pub(crate) fn stream_open_failed(&mut self, id: i32, addressed: &FrameId) -> Option<(C, V)> {
        self.take_stream_open(id, addressed)
            .map(|(owner, promise, _)| (owner.context, promise))
    }

    fn take_stream_open(
        &mut self,
        id: i32,
        addressed: &FrameId,
    ) -> Option<(Owner<C>, V, StreamSink<V>)> {
        match self.pending.get(&id) {
            Some((owner, Pending::StreamOpen { .. })) if owner.frame == *addressed => {}
            _ => return None,
        }
        match self.pending.remove(&id)? {
            (owner, Pending::StreamOpen { promise, sink }) => Some((owner, promise, sink)),
            (_, Pending::Rpc(_)) => None,
        }
    }

    /// The data callback of a stream of `addressed`, open or still opening
    /// (a handler may send data before the open completes).
    pub(crate) fn stream_data(&self, id: i32, addressed: &FrameId) -> Option<(C, V)> {
        if let Some((owner, sink)) = self.streams.get(&id) {
            return (owner.frame == *addressed).then(|| (owner.context.clone(), sink.data.clone()));
        }
        match self.pending.get(&id) {
            Some((owner, Pending::StreamOpen { sink, .. })) if owner.frame == *addressed => {
                Some((owner.context.clone(), sink.data.clone()))
            }
            _ => None,
        }
    }

    /// Closes a stream of `addressed`; returns its end callback.
    pub(crate) fn stream_end(&mut self, id: i32, addressed: &FrameId) -> Option<(C, V)> {
        self.take_stream(id, addressed)
            .map(|(context, sink)| (context, sink.end))
    }

    /// Fails a stream of `addressed`; returns its error callback.
    pub(crate) fn stream_error(&mut self, id: i32, addressed: &FrameId) -> Option<(C, V)> {
        self.take_stream(id, addressed)
            .map(|(context, sink)| (context, sink.error))
    }

    fn take_stream(&mut self, id: i32, addressed: &FrameId) -> Option<(C, StreamSink<V>)> {
        match self.streams.get(&id) {
            Some((owner, _)) if owner.frame == *addressed => {}
            _ => return None,
        }
        self.streams
            .remove(&id)
            .map(|(owner, sink)| (owner.context, sink))
    }

    /// Whether `current` opened the (open) stream `id`.
    pub(crate) fn owns_stream(&self, id: i32, current: &C) -> bool {
        self.streams
            .get(&id)
            .is_some_and(|(owner, _)| owner.context.same(current))
    }

    pub(crate) fn subscribe(
        &mut self,
        context: &C,
        event: String,
        callback: V,
        on_error: Option<V>,
    ) -> Option<i32> {
        let owner = self.owner(context)?;
        let id = self.allocate();
        self.events.insert(
            id,
            (
                owner,
                Subscription {
                    event,
                    callback,
                    on_error,
                },
            ),
        );
        Some(id)
    }

    /// Removes a subscription, only for the context that made it; returns
    /// its event name.
    pub(crate) fn unsubscribe(&mut self, id: i32, current: &C) -> Option<String> {
        let (owner, _) = self.events.get(&id)?;
        if !owner.context.same(current) {
            return None;
        }
        self.events.remove(&id).map(|(_, sub)| sub.event)
    }

    /// The callback of subscription `id`, if `addressed` made it.
    pub(crate) fn event(&self, id: i32, addressed: &FrameId) -> Option<(C, V)> {
        let (owner, sub) = self.events.get(&id)?;
        (owner.frame == *addressed).then(|| (owner.context.clone(), sub.callback.clone()))
    }

    /// The browser refused subscription `id` of `addressed`; removes it and
    /// returns its `onError`.
    pub(crate) fn refused(&mut self, id: i32, addressed: &FrameId) -> Option<(C, Option<V>)> {
        match self.events.get(&id) {
            Some((owner, _)) if owner.frame == *addressed => {}
            _ => return None,
        }
        self.events
            .remove(&id)
            .map(|(owner, sub)| (owner.context, sub.on_error))
    }

    fn known(&self, context: &C) -> Option<&Known<C>> {
        self.contexts
            .iter()
            .find(|known| known.context.same(context))
    }

    fn owner(&self, context: &C) -> Option<Owner<C>> {
        self.known(context).map(|known| Owner {
            context: known.context.clone(),
            frame: known.frame.clone(),
        })
    }

    /// The next id in use by no kind of entry.
    fn allocate(&mut self) -> i32 {
        loop {
            let id = self.next_id;
            self.next_id = self.next_id.checked_add(1).unwrap_or(1);
            if !self.pending.contains_key(&id)
                && !self.streams.contains_key(&id)
                && !self.events.contains_key(&id)
            {
                return id;
            }
        }
    }
}

/// Returns a starting id derived from the renderer process id.
fn random_start() -> i32 {
    use std::hash::{BuildHasher, RandomState};
    let seed = RandomState::new().hash_one(std::process::id());
    (seed as u32 & 0x3FFF_FFFF) as i32 + 1
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Identifier for a V8 context.
    #[derive(Clone, Debug, PartialEq)]
    struct Ctx(u32);

    impl ContextHandle for Ctx {
        fn same(&self, other: &Self) -> bool {
            self == other
        }
    }

    type Values = &'static str;

    fn frame(name: &str) -> FrameId {
        FrameId::new(name)
    }

    fn sink() -> StreamSink<Values> {
        StreamSink {
            data: "data",
            end: "end",
            error: "error",
        }
    }

    /// Frames A and B, one context each, both in this renderer process.
    fn two_frames() -> Registry<Ctx, Values> {
        let mut registry = Registry::new();
        registry.context_created(Ctx(1), frame("A"), false);
        registry.context_created(Ctx(2), frame("B"), false);
        registry
    }

    #[test]
    fn only_the_requesting_context_can_cancel() {
        let mut r = two_frames();
        let id = r.register_rpc(&Ctx(1), "promise").unwrap();
        assert!(r.cancel(id, &Ctx(2)).is_none(), "B cancelled A's request");
        assert_eq!(r.cancel(id, &Ctx(1)), Some((Ctx(1), "promise", SUB_RPC)));
    }

    #[test]
    fn answers_reach_only_the_addressed_frame() {
        let mut r = two_frames();
        let id = r.register_rpc(&Ctx(1), "promise").unwrap();
        assert!(
            r.settle(id, &frame("B")).is_none(),
            "an answer addressed to B took A's promise"
        );
        assert_eq!(r.settle(id, &frame("A")), Some((Ctx(1), "promise")));
        assert!(r.settle(id, &frame("A")).is_none(), "settled twice");
    }

    #[test]
    fn stream_callbacks_are_bound_at_open_and_reach_only_their_frame() {
        let mut r = two_frames();
        let id = r.register_stream_open(&Ctx(1), "open", sink()).unwrap();
        // Data sent before the open completes still reaches the opener
        assert_eq!(r.stream_data(id, &frame("A")), Some((Ctx(1), "data")));
        assert!(r.stream_data(id, &frame("B")).is_none());
        assert!(r.stream_opened(id, &frame("B")).is_none());
        assert_eq!(r.stream_opened(id, &frame("A")), Some((Ctx(1), "open")));
        assert!(r.owns_stream(id, &Ctx(1)) && !r.owns_stream(id, &Ctx(2)));
        assert!(
            r.stream_end(id, &frame("B")).is_none(),
            "B ended A's stream"
        );
        assert_eq!(r.stream_end(id, &frame("A")), Some((Ctx(1), "end")));
        assert!(
            r.stream_data(id, &frame("A")).is_none(),
            "an ended stream stays closed"
        );
    }

    #[test]
    fn only_the_subscribing_context_can_unsubscribe_and_receive() {
        let mut r = two_frames();
        let a = r
            .subscribe(&Ctx(1), "tick".into(), "a-callback", None)
            .unwrap();
        let b = r
            .subscribe(&Ctx(2), "tick".into(), "b-callback", Some("b-error"))
            .unwrap();
        assert!(
            r.unsubscribe(a, &Ctx(2)).is_none(),
            "B removed A's subscription"
        );
        // An emit for A's subscription reaches A's callback only
        assert_eq!(r.event(a, &frame("A")), Some((Ctx(1), "a-callback")));
        assert!(r.event(a, &frame("B")).is_none());
        assert!(r.refused(b, &frame("A")).is_none());
        assert_eq!(r.refused(b, &frame("B")), Some((Ctx(2), Some("b-error"))));
        assert!(
            r.event(b, &frame("B")).is_none(),
            "a refused subscription receives nothing"
        );
        assert_eq!(r.unsubscribe(a, &Ctx(1)).as_deref(), Some("tick"));
    }

    #[test]
    fn a_released_context_takes_everything_with_it() {
        let mut r = two_frames();
        let rpc = r.register_rpc(&Ctx(1), "p").unwrap();
        let open = r.register_stream_open(&Ctx(1), "o", sink()).unwrap();
        let sub = r.subscribe(&Ctx(1), "tick".into(), "cb", None).unwrap();
        r.context_released(&Ctx(1));
        assert!(r.settle(rpc, &frame("A")).is_none());
        assert!(r.stream_data(open, &frame("A")).is_none());
        assert!(r.event(sub, &frame("A")).is_none());
        assert!(
            r.register_rpc(&Ctx(1), "p").is_none(),
            "a released context registers nothing"
        );
    }

    #[test]
    fn unknown_and_opaque_contexts_act_for_no_origin() {
        let mut r: Registry<Ctx, Values> = two_frames();
        r.context_created(Ctx(3), frame("C"), true);
        assert_eq!(r.flags_for(&Ctx(1)), 0);
        assert_eq!(r.flags_for(&Ctx(3)), FLAG_OPAQUE_CONTEXT);
        assert_eq!(r.flags_for(&Ctx(9)), FLAG_OPAQUE_CONTEXT);
    }

    #[test]
    fn ids_are_unique_across_kinds() {
        let mut r = two_frames();
        r.next_id = i32::MAX;
        let first = r.register_rpc(&Ctx(1), "p").unwrap();
        let second = r.subscribe(&Ctx(1), "e".into(), "cb", None).unwrap();
        let third = r.register_stream_open(&Ctx(2), "o", sink()).unwrap();
        assert_eq!(first, i32::MAX);
        assert_eq!(second, 1, "ids wrap to 1, never to zero or a negative");
        assert!(third != first && third != second);
    }
}
