//! Per-frame order for the browser's messages to a renderer.
//!
//! A large ArrayBuffer for the page is filled on another thread. The frame's
//! later messages wait behind it, so each frame receives its messages in the
//! order the browser sent them. The bytes being filled have a cap, past which
//! a message is copied on the main thread instead.
//!
//! A fill moves its message to the filler thread and back on one long-lived
//! channel, so no state is shared between the threads and no hand-off
//! allocates one of its own.
//!
//! Generic over the frame, message and store so the rules can be tested
//! without CEF. `renderer_delivery` supplies CEF's types.

use std::collections::{HashMap, VecDeque};
use std::sync::mpsc::{Receiver, Sender, channel};

use crate::ipc::FrameId;

/// Smallest ArrayBuffer filled off the main thread. Below it the two thread
/// hops cost more than the copy they move. Measured on Windows, Linux and
/// macOS, one awaited 256 KiB result came back 5 to 11 percent slower filled
/// off the main thread, a 1 MiB one level.
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
    /// Nothing waits before it, so it is routed now.
    Route(M),
    /// It waits behind its frame's earlier messages.
    Waits,
    /// It waits for this job, which fills its store on another thread.
    Fill(Job<M, S>),
}

/// Each frame's messages waiting for a fill before them.
pub(crate) struct Inbox<F, M, S> {
    queues: HashMap<FrameId, VecDeque<Entry<F, M, S>>>,
    /// Bytes of fills not delivered yet.
    filling: usize,
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
            next_fill: 0,
            back,
            returned,
            wake,
        }
    }

    /// Takes a message from the browser. It is routed at once when its frame
    /// has nothing waiting and it needs no fill; else it waits in order.
    /// `frame` gives its frame's id and the handle to route it with. `store`
    /// makes the store an ArrayBuffer of `len` bytes is filled into. Each runs
    /// only when needed, since both cost a call into CEF.
    pub(crate) fn arrive(
        &mut self,
        frame: impl FnOnce() -> (FrameId, F),
        message: M,
        len: usize,
        store: impl FnOnce(usize) -> Option<S>,
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
    /// the store its fill wrote.
    pub(crate) fn next(&mut self) -> Option<(F, M, Option<S>)> {
        // Every fill keeps an entry until it is delivered, so no return is on
        // its way to an empty inbox
        if self.queues.is_empty() {
            return None;
        }
        while let Ok(returned) = self.returned.try_recv() {
            self.settle(returned);
        }
        let (id, frame, message, store, filling) =
            self.queues.iter_mut().find_map(|(id, queue)| {
                let entry = queue.pop_front()?;
                match entry.waiting {
                    Waiting::Ready(message, store) => {
                        Some((id.clone(), entry.frame, message, store, entry.filling))
                    }
                    // Its fill still runs
                    Waiting::Filling(fill) => {
                        queue.push_front(Entry {
                            waiting: Waiting::Filling(fill),
                            ..entry
                        });
                        None
                    }
                }
            })?;
        if self.queues.get(&id).is_some_and(VecDeque::is_empty) {
            self.queues.remove(&id);
        }
        self.filling -= filling;
        Some((frame, message, store))
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
        )
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
        assert!(inbox.next().is_none());
        job.run(|_, store| {
            store.fill(7);
            true
        });
        assert_eq!(inbox.next(), Some(("a", 1, Some(vec![7; 4]))));
        assert_eq!(inbox.next(), Some(("a", 2, None)));
        assert!(inbox.next().is_none());
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
        assert_eq!(inbox.next(), Some(("a", 1, None)));
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
