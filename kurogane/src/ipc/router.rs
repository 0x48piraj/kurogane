//! IPC message router
//!
//! Central dispatch layer for all IPC messages between browser and renderer.
//! Owns the per-subsystem handler maps and the ACL, which it applies once,
//! here, before any subsystem sees a message.

use cef::*;

use crate::acl::{CommandAcl, Origin};
use crate::browser_registry::BrowserId;
use crate::debug;
use crate::ipc::browser_state::{ErrorCode, FrameId, IpcContext, IpcError};
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

/// What a message opens: a command invocation, a stream or an event
/// subscription, whose sender waits for an answer (the invocation's promise,
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
pub fn route_renderer(frame: &mut Frame, envelope: &Envelope, payload: &[u8]) -> bool {
    match envelope.subsystem {
        SUB_RPC => crate::ipc::rpc::renderer::handle_rpc_renderer(frame, envelope, payload),
        SUB_EVENT => crate::ipc::event::renderer::handle_event_renderer(frame, envelope, payload),
        SUB_STREAM => {
            crate::ipc::stream::renderer::handle_stream_renderer(frame, envelope, payload)
        }
        _ => {
            debug!("[Router Renderer] unknown subsystem {}", envelope.subsystem);
            false
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
        }
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
        if self.refuse(frame, envelope, payload, &ctx.origin) {
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

    /// Answers a message the ACL refuses, in the form its caller awaits:
    /// an RPC rejection, a failed stream open, or a refused subscription.
    /// All carry [`ErrorCode::Acl`]. Returns true when the message was refused.
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

    /// Drops everything opened by the document in `frame`. Pending requests are
    /// cancelled and their late responses suppressed; subscriptions and streams
    /// are removed. State never carries into the next document, regardless of
    /// origin. State belonging to invalid frames is swept at the same time.
    pub fn clear_for_frame(&self, frame: &FrameId) {
        let requests = self.request_response.pending.cancel_frame(frame);
        let events = self.event.clear_frame(frame);
        let streams = self.stream.clear_frame(frame);
        if requests + events + streams > 0 {
            debug!(
                "[Router] frame started a new document: cancelled {} requests, removed {} event subs, {} streams",
                requests, events, streams
            );
        }

        let events = self.event.clear_invalid_frames();
        let streams = self.stream.clear_invalid_frames();
        if events + streams > 0 {
            debug!(
                "[Router] cleaned up {} event subs, {} streams for invalid frames",
                events, streams
            );
        }
    }

    /// Cancel all pending async handlers for a given browser.
    pub fn cancel_all_for_browser(&self, browser_id: BrowserId) -> usize {
        let req_count = self
            .request_response
            .pending
            .cancel_all_for_browser(browser_id);
        // Clean up event subscriptions and stream state
        let events = self.event.clear_for_browser(browser_id);
        let streams = self.stream.clear_for_browser(browser_id);
        if req_count + events + streams > 0 {
            debug!(
                "[Router] browser closed: canceled {} pending handlers, removed {} event subs, {} streams",
                req_count, events, streams
            );
        }
        req_count
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
