//! Per-frame order for the browser's messages to a renderer.
//!
//! A large ArrayBuffer for the page is filled on another thread. A full
//! batch (the sign of a burst) is delivered in a task of its own. The page's
//! own tasks run between a burst's batches. The frame's later messages wait
//! behind either. Each frame receives its messages in the order the browser
//! sent them. The bytes being filled have a cap. Past it a message is copied
//! on the main thread instead.
//!
//! A fill moves its message to the filler thread and back on one long-lived
//! channel. No state is shared between the threads and no hand-off allocates
//! a channel of its own.
//!
//! Generic over the frame, message and store so the rules can be tested
//! without CEF. `renderer_delivery` supplies CEF's types.

use std::collections::{HashMap, VecDeque};
use std::sync::mpsc::{Receiver, Sender, channel};

use crate::ipc::FrameId;

/// Smallest ArrayBuffer filled off the main thread. Below it the two thread
/// hops cost more than the copy they move.
pub(crate) const FILL_MIN: usize = 1024 * 1024;

/// Most bytes being filled or waiting for delivery at once. A single larger
/// message still fills when nothing else does.
pub(crate) const FILL_CAP: usize = 64 * 1024 * 1024;

/// Most fills the cap lets wait at once, the bound of the filler's queue.
pub(crate) const MAX_FILLS: usize = FILL_CAP / FILL_MIN;

/// A message back from its fill, with its store when the fill wrote it.
struct Returned<M, S> {
    fill: u64,
    message: M,
    store: Option<S>,
}

/// A message in its frame's queue.
enum Waiting<M, S> {
    Ready(M, Option<S>),
    /// Ready, for a delivery task of its own.
    Deferred(M),
    /// It waits for the return of this fill.
    Filling(u64),
}

struct Entry<F, M, S> {
    frame: F,
    waiting: Waiting<M, S>,
    /// Bytes its fill counts against the cap.
    filling: usize,
}

/// What becomes of a message from the browser.
pub(crate) enum Arrival<M, S> {
    /// Nothing waits before it. It is routed now.
    Route(M),
    /// It waits behind its frame's earlier messages.
    Waits,
    /// It waits for a delivery task of its own.
    Defer,
    /// It waits for this job, which fills its store on another thread.
    Fill(Job<M, S>),
}

/// Each frame's messages waiting for a fill before them.
pub(crate) struct Inbox<F, M, S> {
    queues: HashMap<FrameId, VecDeque<Entry<F, M, S>>>,
    /// Bytes of fills not delivered yet.
    filling: usize,
    /// Messages that wait for a delivery task of their own.
    deferred: usize,
    /// The number of the next fill.
    next_fill: u64,
    /// Every job hands its message back on this channel. Unbounded, yet the
    /// cap bounds the fills that can return.
    back: Sender<Returned<M, S>>,
    returned: Receiver<Returned<M, S>>,
    /// Wakes the main thread to deliver a message a fill handed back.
    wake: fn(),
}

impl<F, M, S> Inbox<F, M, S> {
    /// An empty inbox whose fills wake the main thread with `wake`.
    pub(crate) fn new(wake: fn()) -> Self {
        let (back, returned) = channel();
        Self {
            queues: HashMap::new(),
            filling: 0,
            deferred: 0,
            next_fill: 0,
            back,
            returned,
            wake,
        }
    }

    /// Takes a message from the browser. It is routed at once when its frame
    /// has nothing waiting, it needs no fill and it is not `deferred`; else
    /// it waits in order. `frame` gives its frame's id and the handle to
    /// route it with. `store` makes the store an ArrayBuffer of `len` bytes
    /// is filled into. Each runs only when needed, since both cost a call
    /// into CEF.
    pub(crate) fn arrive(
        &mut self,
        frame: impl FnOnce() -> (FrameId, F),
        message: M,
        len: usize,
        store: impl FnOnce(usize) -> Option<S>,
        deferred: bool,
    ) -> Arrival<M, S> {
        let fills = len >= FILL_MIN && (self.filling == 0 || self.filling + len <= FILL_CAP);
        if let Some(store) = fills.then(|| store(len)).flatten() {
            let (id, frame) = frame();
            let fill = self.next_fill;
            self.next_fill += 1;
            self.filling += len;
            self.queues.entry(id).or_default().push_back(Entry {
                frame,
                waiting: Waiting::Filling(fill),
                filling: len,
            });
            return Arrival::Fill(Job {
                fill,
                work: Some((message, store)),
                back: self.back.clone(),
                wake: self.wake,
            });
        }
        if deferred {
            self.deferred += 1;
            let (id, frame) = frame();
            self.queues.entry(id).or_default().push_back(Entry {
                frame,
                waiting: Waiting::Deferred(message),
                filling: 0,
            });
            return Arrival::Defer;
        }
        if self.queues.is_empty() {
            return Arrival::Route(message);
        }
        let (id, frame) = frame();
        match self.queues.get_mut(&id) {
            Some(queue) => {
                queue.push_back(Entry {
                    frame,
                    waiting: Waiting::Ready(message, None),
                    filling: 0,
                });
                Arrival::Waits
            }
            None => Arrival::Route(message),
        }
    }

    /// Takes the first frame's front message that nothing holds back, with
    /// the store its fill wrote and whether it was deferred. A deferred one
    /// only when `deferred`.
    pub(crate) fn next(&mut self, deferred: bool) -> Option<(F, M, Option<S>, bool)> {
        // Every fill keeps an entry until it is delivered, so no return is on
        // its way to an empty inbox
        if self.queues.is_empty() {
            return None;
        }
        while let Ok(returned) = self.returned.try_recv() {
            self.settle(returned);
        }
        let (id, frame, message, store, filling, was_deferred) =
            self.queues.iter_mut().find_map(|(id, queue)| {
                let entry = queue.pop_front()?;
                match entry.waiting {
                    Waiting::Ready(message, store) => Some((
                        id.clone(),
                        entry.frame,
                        message,
                        store,
                        entry.filling,
                        false,
                    )),
                    Waiting::Deferred(message) if deferred => {
                        Some((id.clone(), entry.frame, message, None, entry.filling, true))
                    }
                    // Its own task, or its fill, still to come
                    waiting @ (Waiting::Deferred(_) | Waiting::Filling(_)) => {
                        queue.push_front(Entry { waiting, ..entry });
                        None
                    }
                }
            })?;
        if self.queues.get(&id).is_some_and(VecDeque::is_empty) {
            self.queues.remove(&id);
        }
        self.filling -= filling;
        self.deferred -= usize::from(was_deferred);
        Some((frame, message, store, was_deferred))
    }

    /// Puts the rest of a message back in front of its frame's queue, for a
    /// delivery task of its own.
    pub(crate) fn resume(&mut self, id: FrameId, frame: F, message: M) {
        self.deferred += 1;
        self.queues.entry(id).or_default().push_front(Entry {
            frame,
            waiting: Waiting::Deferred(message),
            filling: 0,
        });
    }

    /// Returns whether a message waits for a delivery task of its own.
    pub(crate) fn has_deferred(&self) -> bool {
        self.deferred > 0
    }

    /// Puts a returned message in the entry that waits for its fill. Every
    /// fill's entry waits until its return arrives.
    fn settle(&mut self, returned: Returned<M, S>) {
        let waiting = self
            .queues
            .values_mut()
            .flatten()
            .map(|entry| &mut entry.waiting)
            .find(|waiting| matches!(waiting, Waiting::Filling(fill) if *fill == returned.fill));
        debug_assert!(waiting.is_some(), "a return for no waiting fill");
        if let Some(waiting) = waiting {
            *waiting = Waiting::Ready(returned.message, returned.store);
        }
    }
}

/// A fill for another thread. It hands its message back however it ends,
/// run, refused or dropped, a panic included.
pub(crate) struct Job<M, S> {
    fill: u64,
    work: Option<(M, S)>,
    back: Sender<Returned<M, S>>,
    wake: fn(),
}

impl<M, S> Job<M, S> {
    /// Runs `write` on the message and its store, then hands both back. A
    /// store `write` refused stays behind.
    pub(crate) fn run(mut self, write: impl FnOnce(&M, &mut S) -> bool) {
        let Some((message, store)) = self.work.as_mut() else {
            return;
        };
        let written = write(message, store);
        if let Some((message, store)) = self.work.take() {
            self.hand_back(message, written.then_some(store));
        }
    }

    /// Sends the message back and wakes the main thread. An inbox already
    /// gone takes nothing back and needs no wake-up.
    fn hand_back(&self, message: M, store: Option<S>) {
        let returned = Returned {
            fill: self.fill,
            message,
            store,
        };
        if self.back.send(returned).is_ok() {
            (self.wake)();
        }
    }
}

impl<M, S> Drop for Job<M, S> {
    fn drop(&mut self) {
        if let Some((message, _)) = self.work.take() {
            self.hand_back(message, None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestInbox = Inbox<&'static str, u32, Vec<u8>>;

    fn inbox() -> TestInbox {
        Inbox::new(|| {})
    }

    fn arrive(
        inbox: &mut TestInbox,
        frame: &'static str,
        message: u32,
        len: usize,
    ) -> Arrival<u32, Vec<u8>> {
        inbox.arrive(
            || (FrameId::new(frame), frame),
            message,
            len,
            |len| Some(vec![0; len.min(4)]),
            false,
        )
    }

    fn defer(inbox: &mut TestInbox, frame: &'static str, message: u32) -> Arrival<u32, Vec<u8>> {
        inbox.arrive(|| (FrameId::new(frame), frame), message, 10, |_| None, true)
    }

    fn job(arrival: Arrival<u32, Vec<u8>>) -> Job<u32, Vec<u8>> {
        match arrival {
            Arrival::Fill(job) => job,
            _ => panic!("no fill"),
        }
    }

    #[test]
    fn a_message_waits_for_the_fill_before_it() {
        let mut inbox = inbox();
        let job = job(arrive(&mut inbox, "a", 1, FILL_MIN));
        assert!(matches!(arrive(&mut inbox, "a", 2, 10), Arrival::Waits));
        assert!(inbox.next(true).is_none());
        job.run(|_, store| {
            store.fill(7);
            true
        });
        assert_eq!(inbox.next(true), Some(("a", 1, Some(vec![7; 4]), false)));
        assert_eq!(inbox.next(true), Some(("a", 2, None, false)));
        assert!(inbox.next(true).is_none());
    }

    #[test]
    fn another_frame_does_not_wait() {
        let mut inbox = inbox();
        let _job = job(arrive(&mut inbox, "a", 1, FILL_MIN));
        assert!(matches!(arrive(&mut inbox, "b", 2, 10), Arrival::Route(2)));
    }

    #[test]
    fn a_dropped_job_hands_its_message_back_without_the_store() {
        let mut inbox = inbox();
        drop(job(arrive(&mut inbox, "a", 1, FILL_MIN)));
        assert_eq!(inbox.next(true), Some(("a", 1, None, false)));
    }

    #[test]
    fn a_deferred_message_waits_for_a_task_and_its_frame_waits_behind_it() {
        let mut inbox = inbox();
        assert!(matches!(defer(&mut inbox, "a", 1), Arrival::Defer));
        assert!(matches!(arrive(&mut inbox, "a", 2, 10), Arrival::Waits));
        assert!(matches!(arrive(&mut inbox, "b", 3, 10), Arrival::Route(3)));
        assert!(inbox.has_deferred());
        assert!(
            inbox.next(false).is_none(),
            "only a task of its own takes it"
        );
        assert_eq!(inbox.next(true), Some(("a", 1, None, true)));
        assert!(!inbox.has_deferred());
        assert_eq!(inbox.next(false), Some(("a", 2, None, false)));
        assert!(inbox.next(true).is_none());
    }

    #[test]
    fn a_fill_behind_a_deferred_message_waits_for_it() {
        let mut inbox = inbox();
        assert!(matches!(defer(&mut inbox, "a", 1), Arrival::Defer));
        let job = job(arrive(&mut inbox, "a", 2, FILL_MIN));
        drop(job);
        assert!(inbox.next(false).is_none(), "the fill came back first");
        assert_eq!(inbox.next(true), Some(("a", 1, None, true)));
        assert_eq!(inbox.next(false), Some(("a", 2, None, false)));
    }

    #[test]
    fn a_deferred_message_behind_a_fill_waits_for_it() {
        let mut inbox = inbox();
        let job = job(arrive(&mut inbox, "a", 1, FILL_MIN));
        assert!(matches!(defer(&mut inbox, "a", 2), Arrival::Defer));
        assert!(inbox.next(true).is_none(), "the fill runs");
        job.run(|_, _| true);
        assert_eq!(inbox.next(false), Some(("a", 1, Some(vec![0; 4]), false)));
        assert!(inbox.next(false).is_none());
        assert!(inbox.has_deferred());
        assert_eq!(inbox.next(true), Some(("a", 2, None, true)));
        assert!(!inbox.has_deferred());
    }

    #[test]
    fn a_resumed_message_goes_first_in_a_task_of_its_own() {
        let mut inbox = inbox();
        assert!(matches!(defer(&mut inbox, "a", 1), Arrival::Defer));
        assert!(matches!(arrive(&mut inbox, "a", 2, 10), Arrival::Waits));
        assert_eq!(inbox.next(true), Some(("a", 1, None, true)));
        inbox.resume(FrameId::new("a"), "a", 10);
        assert!(inbox.has_deferred());
        assert!(matches!(arrive(&mut inbox, "a", 3, 10), Arrival::Waits));
        assert!(inbox.next(false).is_none(), "the rest waits for its task");
        assert_eq!(inbox.next(true), Some(("a", 10, None, true)));
        assert!(!inbox.has_deferred());
        assert_eq!(inbox.next(false), Some(("a", 2, None, false)));
        assert_eq!(inbox.next(false), Some(("a", 3, None, false)));
        inbox.resume(FrameId::new("b"), "b", 20);
        assert!(matches!(arrive(&mut inbox, "b", 4, 10), Arrival::Waits));
        assert_eq!(inbox.next(true), Some(("b", 20, None, true)));
    }

    #[test]
    fn past_the_cap_a_message_is_copied_in_order() {
        let mut inbox = inbox();
        let _first = job(arrive(&mut inbox, "a", 1, FILL_CAP * 2));
        assert!(matches!(
            arrive(&mut inbox, "a", 2, FILL_MIN),
            Arrival::Waits
        ));
        assert!(matches!(
            arrive(&mut inbox, "b", 3, FILL_MIN),
            Arrival::Route(3)
        ));
    }
}
