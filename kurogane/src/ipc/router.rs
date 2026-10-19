//! IPC message router
//!
//! Central dispatch layer for all IPC messages between browser and renderer.
//! Owns the per-subsystem handler maps, the ACL and the documents messages
//! come from. Applies the ACL once, here, before any subsystem sees a
//! message.

use std::sync::Arc;

use std::time::Instant;

use tanso::*;

use crate::acl::{CommandAcl, Origin};
use crate::browser_registry::BrowserId;
use tracing::debug;
use crate::ipc::batch;
use crate::ipc::browser_state::{ErrorCode, FrameId, IpcContext, IpcError};
use crate::ipc::document::{Document, Documents};
use crate::ipc::envelope::*;
use crate::ipc::request_response::RequestResponseSubsystem;
use crate::ipc::event::EventSubsystem;
use crate::ipc::stream::{StreamResponder, StreamSubsystem};
use crate::runtime::AppHandle;

/// Top-level IPC router that owns all subsystems.
pub struct IpcRouter {
    pub request_response: RequestResponseSubsystem,
    pub event: EventSubsystem,
    pub stream: StreamSubsystem,
    acl: CommandAcl,
    documents: Documents,
}

/// The ACL's answer for one incoming message.
#[derive(Debug, PartialEq, Eq)]
enum Verdict<'p> {
    Pass,
    /// A command invocation or stream open of this name, refused.
    RefuseName(&'p str),
    /// A subscription to this event, refused.
    RefuseEvent(&'p str),
}

/// What a message opens. The sender of a command invocation, a stream or an
/// event subscription waits for an answer (the invocation's promise,
/// `openStream`'s promise, the subscription's `onError`). Follow-ups
/// (cancel, stream data, end, error and cancel, unsubscribe) open nothing
/// and wait for nothing.
#[derive(Debug, PartialEq, Eq)]
enum Opens {
    Invocation,
    Stream,
    Subscription,
}

fn opens(envelope: &Envelope) -> Option<Opens> {
    match (envelope.subsystem, envelope.opcode) {
        (SUB_RPC, RPC_INVOKE) => Some(Opens::Invocation),
        (SUB_STREAM, STREAM_OPEN) => Some(Opens::Stream),
        (SUB_EVENT, EVENT_SUBSCRIBE) => Some(Opens::Subscription),
        _ => None,
    }
}

/// Applies the ACL to messages that open commands, streams or event
/// subscriptions. Follow-up messages such as cancel, stream data and
/// unsubscribe are authorized by their subsystems against the frame and
/// origin that opened them. Malformed payloads pass through and are rejected
/// by the corresponding subsystem.
fn verdict<'p>(
    acl: &CommandAcl,
    envelope: &Envelope,
    payload: &'p [u8],
    origin: &Origin,
) -> Verdict<'p> {
    match opens(envelope) {
        Some(Opens::Invocation | Opens::Stream) => match decode_cmd_payload(payload) {
            Some((name, _)) if !acl.allows(name, origin) => Verdict::RefuseName(name),
            _ => Verdict::Pass,
        },
        Some(Opens::Subscription) => match decode_cmd_payload(payload) {
            Some((name, _)) if !acl.allows_event(name, origin) => Verdict::RefuseEvent(name),
            _ => Verdict::Pass,
        },
        None => Verdict::Pass,
    }
}

/// Refuses what `envelope` opens with `error`, in the form its sender waits
/// for. A follow-up waits for nothing and gets nothing.
fn refuse_opening(frame: &Frame, envelope: &Envelope, error: IpcError) {
    match opens(envelope) {
        Some(Opens::Invocation) => RequestResponseSubsystem::reject(frame, envelope, error),
        Some(Opens::Stream) => {
            let _ = StreamResponder::unbound(frame.clone(), envelope.correlation_id).error(error);
        }
        Some(Opens::Subscription) => EventSubsystem::refuse(frame, envelope, &error),
        None => {}
    }
}

/// Standalone renderer-side dispatcher.
///
/// Routes a decoded envelope + payload to the appropriate subsystem handler.
/// Needs no [`IpcRouter`] which exists only in the browser process.
/// `filled` already holds the bytes of the ArrayBuffer the message makes;
/// `addressed` is the frame it reached.
pub fn route_renderer(
    addressed: &FrameId,
    envelope: &Envelope,
    payload: &[u8],
    filled: Option<V8BackingStore>,
) -> bool {
    match envelope.subsystem {
        SUB_RPC => {
            crate::ipc::rpc::renderer::handle_rpc_renderer(addressed, envelope, payload, filled)
        }
        SUB_EVENT => {
            crate::ipc::event::renderer::handle_event_renderer(addressed, envelope, payload, filled)
        }
        SUB_STREAM => crate::ipc::stream::renderer::handle_stream_renderer(
            addressed, envelope, payload, filled,
        ),
        SUB_BATCH => {
            route_batch(addressed, payload, 0, None);
            true
        }
        _ => {
            debug!("[Router Renderer] unknown subsystem {}", envelope.subsystem);
            false
        }
    }
}

/// Routes the messages of a batch from byte `from` to `addressed`, each as if
/// it came alone, until `deadline` passes. Returns where the rest starts
/// when it passed. At least one message goes each time. A run of answers to
/// calls settles under one enter of its context
/// ([`settle_answers`](crate::ipc::rpc::renderer::settle_answers)).
pub(crate) fn route_batch(
    addressed: &FrameId,
    payload: &[u8],
    from: usize,
    deadline: Option<Instant>,
) -> Option<usize> {
    let is_answer =
        |e: &Envelope| e.subsystem == SUB_RPC && matches!(e.opcode, RPC_RESOLVE | RPC_REJECT);
    let in_time = || deadline.is_none_or(|deadline| Instant::now() < deadline);
    let mut items = batch::Items::at(payload, from);
    let mut first = true;
    loop {
        if !first && !in_time() {
            return items.peek().map(|_| items.pos());
        }
        first = false;
        let (envelope, data) = items.next()?;
        if is_answer(&envelope) {
            let more = std::iter::from_fn(|| {
                let answer = in_time() && items.peek().is_some_and(|(e, _)| is_answer(&e));
                answer.then(|| items.next()).flatten()
            });
            crate::ipc::rpc::renderer::settle_answers(
                addressed,
                std::iter::once((envelope, data)).chain(more),
            );
        } else if envelope.subsystem == SUB_BATCH {
            debug!("[Router Renderer] batch inside a batch dropped");
        } else {
            route_renderer(addressed, &envelope, data, None);
        }
    }
}

impl IpcRouter {
    pub(crate) fn new(
        request_response: RequestResponseSubsystem,
        event: EventSubsystem,
        stream: StreamSubsystem,
        acl: CommandAcl,
    ) -> Self {
        Self {
            request_response,
            event,
            stream,
            acl,
            documents: Documents::default(),
        }
    }

    /// The origin of the application's start page, which the ACL admits to
    /// every name without a rule.
    pub(crate) fn app_origin(&self) -> &Origin {
        self.acl.app_origin()
    }

    /// Route a message received from the renderer (browser-side dispatch).
    pub fn route_browser(
        &self,
        app: &AppHandle,
        frame: &mut Frame,
        envelope: &Envelope,
        payload: &[u8],
        ctx: IpcContext,
    ) -> bool {
        if self.refuse(frame, envelope, payload, ctx.origin()) {
            return true;
        }
        match envelope.subsystem {
            SUB_RPC => self
                .request_response
                .handle_browser(app, frame, envelope, payload, ctx),
            SUB_EVENT => self.event.handle_browser(frame, envelope, payload, ctx),
            SUB_STREAM => self
                .stream
                .handle_browser(app, frame, envelope, payload, ctx),
            _ => {
                debug!("[Router Browser] unknown subsystem {}", envelope.subsystem);
                false
            }
        }
    }

    /// Answers a message the ACL refuses in the form its caller awaits (an
    /// RPC rejection, a failed stream open or a refused subscription). All
    /// carry [`ErrorCode::Acl`]. Returns true when the message was refused.
    fn refuse(&self, frame: &Frame, envelope: &Envelope, payload: &[u8], origin: &Origin) -> bool {
        let message = match verdict(&self.acl, envelope, payload, origin) {
            Verdict::Pass => return false,
            Verdict::RefuseName(name) => {
                debug!(
                    "[Router Browser] ACL denied '{}' for origin {}",
                    name, origin
                );
                format!("'{name}' is not allowed for origin {origin}")
            }
            Verdict::RefuseEvent(name) => {
                debug!(
                    "[Router Browser] ACL denied event '{}' for origin {}",
                    name, origin
                );
                format!("event '{name}' is not allowed for origin {origin}")
            }
        };
        refuse_opening(
            frame,
            envelope,
            IpcError::with_code(message, ErrorCode::Acl),
        );
        true
    }

    /// Answers a message from a browser the runtime has not registered, yet
    /// or any longer. Nothing is dispatched for it; what it opens is refused
    /// so its sender does not wait forever.
    pub(crate) fn refuse_unregistered(frame: &Frame, envelope: &Envelope) {
        refuse_opening(frame, envelope, IpcError::new("the browser is not open"));
    }

    /// Returns the document `frame` shows, for a message it sent. A frame
    /// that has gone shows none.
    pub(crate) fn document(
        &self,
        frame: &Frame,
        frame_id: &FrameId,
        browser: BrowserId,
    ) -> Option<Arc<Document>> {
        self.documents.current(frame, frame_id, browser)
    }

    /// Starts the document `frame` commits, ending the one it replaces.
    pub(crate) fn document_committed(&self, frame: &Frame, browser: BrowserId) {
        if let Some(ended) = self.documents.commit(frame, browser) {
            self.end(&ended, "another document committed in its frame");
        }
    }

    /// Ends the document of `frame`, which has gone.
    pub(crate) fn frame_gone(&self, frame: &Frame) {
        if let Some(ended) = self.documents.frame_gone(frame) {
            self.end(&ended, "its frame has gone");
        }
    }

    /// Ends every document of `browser`, which has closed.
    pub(crate) fn browser_closed(&self, browser: BrowserId) {
        for ended in self.documents.browser_closed(browser) {
            self.end(&ended, "its browser has closed");
        }
    }

    /// Drops everything `document` opened, which has ended. Pending requests
    /// are cancelled and their late responses suppressed; subscriptions and
    /// streams are removed. Nothing carries into a later document.
    fn end(&self, document: &Document, why: &str) {
        let requests = self
            .request_response
            .pending
            .cancel_document(document.key());
        let events = self.event.remove_document(document.key());
        let streams = self.stream.close_document(document.key());
        if requests + events + streams > 0 {
            debug!(
                "[Router] a document ended, {why}: cancelled {requests} requests, removed {events} event subs, {streams} streams"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn envelope(subsystem: u8, opcode: u8) -> Envelope {
        Envelope {
            version: ENVELOPE_VERSION,
            subsystem,
            opcode,
            flags: 0,
            correlation_id: 1,
            payload_kind: PAYLOAD_JSON,
        }
    }

    fn named(name: &str) -> Vec<u8> {
        encode_cmd_payload(name, b"{}").unwrap()
    }

    fn origin(text: &str) -> Origin {
        Origin::parse(text).unwrap()
    }

    /// `cmd` and the event `tick` only for `app://app`; everything else denied.
    fn strict_acl() -> CommandAcl {
        let mut acl = CommandAcl::new(origin("app://app"));
        acl.allow("cmd", [origin("app://app")]).unwrap();
        acl.allow_event("tick", [origin("app://app")]).unwrap();
        acl.deny_unlisted();
        acl
    }

    #[test]
    fn invokes_and_stream_opens_are_gated_by_name() {
        let acl = strict_acl();
        let (app, evil) = (origin("app://app"), origin("https://evil.example"));
        for (subsystem, opcode) in [(SUB_RPC, RPC_INVOKE), (SUB_STREAM, STREAM_OPEN)] {
            let open = envelope(subsystem, opcode);
            assert_eq!(verdict(&acl, &open, &named("cmd"), &app), Verdict::Pass);
            assert_eq!(
                verdict(&acl, &open, &named("cmd"), &evil),
                Verdict::RefuseName("cmd")
            );
            assert_eq!(
                verdict(&acl, &open, &named("other"), &app),
                Verdict::RefuseName("other")
            );
        }
    }

    #[test]
    fn subscriptions_are_gated_by_event() {
        let acl = strict_acl();
        let subscribe = envelope(SUB_EVENT, EVENT_SUBSCRIBE);
        assert_eq!(
            verdict(&acl, &subscribe, &named("tick"), &origin("app://app")),
            Verdict::Pass
        );
        assert_eq!(
            verdict(
                &acl,
                &subscribe,
                &named("tick"),
                &origin("https://evil.example")
            ),
            Verdict::RefuseEvent("tick")
        );
        assert_eq!(
            verdict(&acl, &subscribe, &named("cmd"), &origin("app://app")),
            Verdict::RefuseEvent("cmd"),
            "a command rule is not an event rule"
        );
    }

    #[test]
    fn follow_ups_and_malformed_payloads_pass_to_their_subsystem() {
        let acl = strict_acl();
        let evil = origin("https://evil.example");
        for (subsystem, opcode) in [
            (SUB_RPC, RPC_CANCEL),
            (SUB_STREAM, STREAM_DATA),
            (SUB_STREAM, STREAM_END),
            (SUB_STREAM, STREAM_CANCEL),
            (SUB_EVENT, EVENT_UNSUBSCRIBE),
        ] {
            assert_eq!(
                verdict(&acl, &envelope(subsystem, opcode), &named("cmd"), &evil),
                Verdict::Pass
            );
        }
        assert_eq!(
            verdict(&acl, &envelope(SUB_RPC, RPC_INVOKE), b"\x09", &evil),
            Verdict::Pass
        );
    }

    #[test]
    fn only_messages_that_open_something_wait_for_an_answer() {
        assert_eq!(
            opens(&envelope(SUB_RPC, RPC_INVOKE)),
            Some(Opens::Invocation)
        );
        assert_eq!(
            opens(&envelope(SUB_STREAM, STREAM_OPEN)),
            Some(Opens::Stream)
        );
        assert_eq!(
            opens(&envelope(SUB_EVENT, EVENT_SUBSCRIBE)),
            Some(Opens::Subscription)
        );
        for (subsystem, opcode) in [
            (SUB_RPC, RPC_CANCEL),
            (SUB_STREAM, STREAM_DATA),
            (SUB_STREAM, STREAM_END),
            (SUB_STREAM, STREAM_ERROR),
            (SUB_STREAM, STREAM_CANCEL),
            (SUB_EVENT, EVENT_UNSUBSCRIBE),
            // A subsystem the runtime does not know opens nothing
            (9, RPC_INVOKE),
        ] {
            assert_eq!(
                opens(&envelope(subsystem, opcode)),
                None,
                "subsystem {subsystem}, opcode {opcode}"
            );
        }
    }
}
