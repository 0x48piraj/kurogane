//! Renderer-side IPC state for V8 contexts.
//!
//! Each pending promise, stream and event subscription belongs to one
//! context in one frame. Messages are delivered only to the owning frame.
//! Cancellation and unsubscription are limited to the owning context.
//!
//! Releasing a context removes all state owned by it. A context back from
//! the back-forward cache hands back its requests and streams and keeps its
//! subscriptions. Unknown contexts are opaque and use `FLAG_OPAQUE_CONTEXT`.
//!
//! Stream callbacks are set when the stream is opened.
//!
//! Generic over the context handle so the rules can be tested without V8.
//! `renderer_state` supplies the V8 context and value types.

#![deny(clippy::unwrap_used, clippy::expect_used)]

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

/// What a context left open when its page left, by id.
pub(crate) struct Returned<V> {
    /// Requests in flight with their subsystem and promise, taken.
    pub(crate) requests: Vec<(i32, u8, V)>,
    /// Open streams with their error callback, taken.
    pub(crate) streams: Vec<(i32, V)>,
    /// Subscriptions with their event name, kept.
    pub(crate) subscriptions: Vec<(i32, String)>,
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

impl<C> Known<C> {
    /// Returns the envelope flags of the context's messages.
    fn flags(&self) -> u8 {
        if self.opaque { FLAG_OPAQUE_CONTEXT } else { 0 }
    }
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

    /// Whether `context` was recorded as a document whose own origin is
    /// opaque.
    pub(crate) fn is_opaque(&self, context: &C) -> bool {
        self.known(context).is_some_and(|known| known.opaque)
    }

    /// Returns the envelope flags and the frame of a context's messages, from
    /// one lookup. An unknown context sends nothing.
    pub(crate) fn sender(&self, context: &C) -> Option<(u8, FrameId)> {
        let known = self.known(context)?;
        Some((known.flags(), known.frame.clone()))
    }

    /// Registers a call's promise and returns its id, its envelope flags and
    /// its frame, from one lookup of the context.
    pub(crate) fn register_rpc(&mut self, context: &C, promise: V) -> Option<(i32, u8, FrameId)> {
        let known = self.known(context)?;
        let flags = known.flags();
        let frame = known.frame.clone();
        let owner = Owner {
            context: known.context.clone(),
            frame: frame.clone(),
        };
        let id = self.allocate();
        self.pending.insert(id, (owner, Pending::Rpc(promise)));
        Some((id, flags, frame))
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
    /// its promise and subsystem.
    pub(crate) fn cancel(&mut self, id: i32, current: &C) -> Option<(V, u8)> {
        let (owner, _) = self.pending.get(&id)?;
        if !owner.context.same(current) {
            return None;
        }
        let (_, pending) = self.pending.remove(&id)?;
        Some(match pending {
            Pending::Rpc(promise) => (promise, SUB_RPC),
            Pending::StreamOpen { promise, .. } => (promise, SUB_STREAM),
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

    /// Marks stream `id` opened, which makes its callbacks live. Returns the
    /// open promise.
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

    /// The data callback of an open stream of `addressed`. A stream still
    /// opening receives nothing. The browser acknowledges an open first.
    pub(crate) fn stream_data(&self, id: i32, addressed: &FrameId) -> Option<(C, V)> {
        let (owner, sink) = self.streams.get(&id)?;
        (owner.frame == *addressed).then(|| (owner.context.clone(), sink.data.clone()))
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

    /// Takes the requests and streams `context` left open and lists its
    /// subscriptions, each in id order, for a page back from the
    /// back-forward cache.
    pub(crate) fn returned(&mut self, context: &C) -> Returned<V> {
        let mut requests: Vec<_> = owned_ids(&self.pending, context)
            .into_iter()
            .filter_map(|id| self.pending.remove(&id).map(|(_, pending)| (id, pending)))
            .map(|(id, pending)| match pending {
                Pending::Rpc(promise) => (id, SUB_RPC, promise),
                Pending::StreamOpen { promise, .. } => (id, SUB_STREAM, promise),
            })
            .collect();
        requests.sort_by_key(|(id, ..)| *id);
        let mut streams: Vec<_> = owned_ids(&self.streams, context)
            .into_iter()
            .filter_map(|id| self.streams.remove(&id).map(|(_, sink)| (id, sink.error)))
            .collect();
        streams.sort_by_key(|(id, _)| *id);
        let mut subscriptions: Vec<_> = self
            .events
            .iter()
            .filter(|(_, (owner, _))| owner.context.same(context))
            .map(|(id, (_, sub))| (*id, sub.event.clone()))
            .collect();
        subscriptions.sort_by_key(|(id, _)| *id);
        Returned {
            requests,
            streams,
            subscriptions,
        }
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

/// Returns the ids of `entries` that `context` owns.
fn owned_ids<C: ContextHandle, T>(entries: &HashMap<i32, (Owner<C>, T)>, context: &C) -> Vec<i32> {
    entries
        .iter()
        .filter(|(_, (owner, _))| owner.context.same(context))
        .map(|(id, _)| *id)
        .collect()
}

/// Returns a process-specific starting ID.
///
/// Uses `RandomState` to randomize the starting point.
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
        let id = r.register_rpc(&Ctx(1), "promise").unwrap().0;
        assert!(r.cancel(id, &Ctx(2)).is_none(), "B cancelled A's request");
        assert_eq!(r.cancel(id, &Ctx(1)), Some(("promise", SUB_RPC)));
    }

    #[test]
    fn answers_reach_only_the_addressed_frame() {
        let mut r = two_frames();
        let id = r.register_rpc(&Ctx(1), "promise").unwrap().0;
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
        // Nothing but the acknowledgement reaches a stream still opening
        assert!(r.stream_data(id, &frame("A")).is_none());
        assert!(r.stream_end(id, &frame("A")).is_none());
        assert!(r.stream_opened(id, &frame("B")).is_none());
        assert_eq!(r.stream_opened(id, &frame("A")), Some((Ctx(1), "open")));
        assert_eq!(r.stream_data(id, &frame("A")), Some((Ctx(1), "data")));
        assert!(r.stream_data(id, &frame("B")).is_none());
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
        assert!(r.pending.is_empty() && r.streams.is_empty());
    }

    #[test]
    fn a_refused_open_leaves_nothing() {
        let mut r = two_frames();
        let id = r.register_stream_open(&Ctx(1), "open", sink()).unwrap();
        assert!(r.stream_open_failed(id, &frame("B")).is_none());
        assert_eq!(
            r.stream_open_failed(id, &frame("A")),
            Some((Ctx(1), "open"))
        );
        assert!(
            r.stream_opened(id, &frame("A")).is_none(),
            "a refused open never opens"
        );
        assert!(!r.owns_stream(id, &Ctx(1)));
        assert!(r.pending.is_empty() && r.streams.is_empty());
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
        let rpc = r.register_rpc(&Ctx(1), "p").unwrap().0;
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
    fn a_returned_context_hands_back_what_it_left_open() {
        let mut r = two_frames();
        let call = r.register_rpc(&Ctx(1), "call").unwrap().0;
        let opening = r.register_stream_open(&Ctx(1), "opening", sink()).unwrap();
        let open = r.register_stream_open(&Ctx(1), "open", sink()).unwrap();
        r.stream_opened(open, &frame("A"));
        let sub = r.subscribe(&Ctx(1), "tick".into(), "cb", None).unwrap();
        let other = r.register_rpc(&Ctx(2), "b-call").unwrap().0;

        let returned = r.returned(&Ctx(1));
        let mut requests = vec![(call, SUB_RPC, "call"), (opening, SUB_STREAM, "opening")];
        requests.sort_by_key(|(id, ..)| *id);
        assert_eq!(returned.requests, requests);
        assert_eq!(returned.streams, [(open, "error")]);
        assert_eq!(returned.subscriptions, [(sub, "tick".to_owned())]);

        assert!(
            r.settle(call, &frame("A")).is_none(),
            "a returned call is taken"
        );
        assert!(
            r.stream_data(open, &frame("A")).is_none(),
            "a returned stream is closed"
        );
        assert_eq!(
            r.event(sub, &frame("A")),
            Some((Ctx(1), "cb")),
            "a subscription stays"
        );
        assert_eq!(
            r.settle(other, &frame("B")),
            Some((Ctx(2), "b-call")),
            "another context keeps its calls"
        );
    }

    #[test]
    fn opaque_contexts_act_for_no_origin_and_unknown_ones_send_nothing() {
        let mut r: Registry<Ctx, Values> = two_frames();
        r.context_created(Ctx(3), frame("C"), true);
        assert_eq!(r.sender(&Ctx(1)), Some((0, frame("A"))));
        assert_eq!(r.sender(&Ctx(3)), Some((FLAG_OPAQUE_CONTEXT, frame("C"))));
        assert_eq!(r.sender(&Ctx(9)), None);
    }

    #[test]
    fn ids_are_unique_across_kinds() {
        let mut r = two_frames();
        r.next_id = i32::MAX;
        let first = r.register_rpc(&Ctx(1), "p").unwrap().0;
        let second = r.subscribe(&Ctx(1), "e".into(), "cb", None).unwrap();
        let third = r.register_stream_open(&Ctx(2), "o", sink()).unwrap();
        assert_eq!(first, i32::MAX);
        assert_eq!(second, 1, "ids wrap to 1, never to zero or a negative");
        assert!(third != first && third != second);
    }
}
