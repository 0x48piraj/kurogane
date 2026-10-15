//! Browser-side event dispatch.
//!
//! Handles subscription management for renderer processes and delivers emitted
//! events to subscribed frames.

use tanso::*;

use crate::acl::Origin;
use tracing::debug;
use crate::ipc::browser_outbox;
use crate::ipc::browser_state::{still_addressed, FrameId, IpcContext, IpcError};
use crate::ipc::envelope::*;
use crate::ipc::event::EventSubsystem;

impl EventSubsystem {
    /// Handle an event message arriving from the renderer (browser-side dispatch).
    pub fn handle_browser(
        &self,
        frame: &mut Frame,
        envelope: &Envelope,
        payload: &[u8],
        ctx: IpcContext,
    ) -> bool {
        match envelope.opcode {
            EVENT_SUBSCRIBE => self.on_subscribe(frame, envelope, payload, ctx),
            EVENT_UNSUBSCRIBE => self.on_unsubscribe(envelope, payload, ctx),
            _ => {
                debug!("[Event Browser] unknown opcode {}", envelope.opcode);
                false
            }
        }
    }

    fn on_subscribe(
        &self,
        frame: &mut Frame,
        envelope: &Envelope,
        payload: &[u8],
        ctx: IpcContext,
    ) -> bool {
        let (event_name, _metadata) = match decode_cmd_payload(payload) {
            Some(v) => v,
            None => {
                debug!("[Event Browser] invalid subscribe payload");
                return false;
            }
        };

        let browser_id = ctx.browser_id;

        let mut subs = self.subscriptions.lock().unwrap();
        subs.entry(event_name.to_string()).or_default().push(
            crate::ipc::event::EventSubscription {
                id: envelope.correlation_id,
                frame: frame.clone(),
                browser_id,
                frame_id: ctx.frame,
                origin: ctx.origin,
                url_origin: ctx.url_origin,
            },
        );

        debug!(
            "[Event Browser] subscribed '{}' browser={}",
            event_name,
            browser_id.as_u32()
        );
        true
    }

    fn on_unsubscribe(&self, envelope: &Envelope, payload: &[u8], ctx: IpcContext) -> bool {
        let (event_name, _rest) = match decode_cmd_payload(payload) {
            Some(v) => v,
            None => {
                debug!("[Event Browser] invalid unsubscribe payload");
                return false;
            }
        };

        let removed = self.remove_subscription(event_name, envelope.correlation_id, &ctx);
        debug!(
            "[Event Browser] unsubscribe '{}' id={} removed={}",
            event_name, envelope.correlation_id, removed,
        );
        true
    }

    /// Tells the renderer that the subscription carried by `envelope` was
    /// refused with `error`, so its `onError` fires instead of silence.
    pub(crate) fn refuse(frame: &Frame, envelope: &Envelope, error: &IpcError) {
        if frame.is_valid() == 0 {
            return;
        }
        let reply = Envelope {
            version: ENVELOPE_VERSION,
            subsystem: SUB_EVENT,
            opcode: EVENT_REFUSED,
            flags: 0,
            correlation_id: envelope.correlation_id,
            payload_kind: PAYLOAD_BINARY,
        };
        let payload = encode_error_payload(error.code().wire(), error.message());
        browser_outbox::send_owned(frame, &FrameId::of(frame), &reply, payload);
    }

    /// Broadcast an event to all subscribers of a given event name.
    ///
    /// Each subscription gets its own message, addressed by its id, so the
    /// renderer runs exactly the callback of that subscription (which the ACL
    /// admitted) and nothing else in the process. A frame's messages may
    /// travel together ([`browser_outbox`]). A subscription whose frame
    /// now shows another document receives nothing. `payload_kind` decides
    /// whether the page receives an `ArrayBuffer` or a string.
    pub fn broadcast(&self, cmd: &str, data: &[u8], payload_kind: u8) {
        let Some(payload) = encode_cmd_payload(cmd, data) else {
            debug!("[Event Browser] event name exceeds the protocol length limit");
            return;
        };

        // Copied out, so no CEF call runs under the lock
        let targets: Vec<(Frame, FrameId, u32, Origin)> = {
            let subs = self.subscriptions.lock().unwrap();
            let Some(entries) = subs.get(cmd) else {
                return;
            };
            entries
                .iter()
                .map(|sub| {
                    (
                        sub.frame.clone(),
                        sub.frame_id.clone(),
                        sub.id,
                        sub.url_origin.clone(),
                    )
                })
                .collect()
        };
        for (frame, frame_id, id, url_origin) in targets {
            if frame.is_valid() == 0 {
                continue;
            }
            let url: CefString = (&frame.url()).into();
            if !still_addressed(&url_origin, &url.to_string()) {
                continue;
            }
            let envelope = Envelope {
                version: ENVELOPE_VERSION,
                subsystem: SUB_EVENT,
                opcode: EVENT_EMIT,
                flags: 0,
                correlation_id: id,
                payload_kind,
            };
            browser_outbox::send(&frame, &frame_id, &envelope, &payload);
        }
    }
}
