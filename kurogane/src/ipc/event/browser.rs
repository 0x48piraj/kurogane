//! Browser-side event dispatch.
//!
//! Handles subscription management for renderer processes and delivers emitted
//! events to subscribed frames.

use cef::*;

use crate::debug;
use crate::ipc::browser_state::{ErrorCode, IpcContext};
use crate::ipc::envelope::*;
use crate::ipc::event::EventSubsystem;
use crate::ipc::transport::message::build_message;

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

        let browser_id = match ctx.browser_id {
            Some(id) => id,
            None => {
                debug!("[Event Browser] subscribe without browser_id");
                return false;
            }
        };

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
    /// refused by the ACL, so its `onError` fires instead of silence.
    pub(crate) fn refuse(frame: &Frame, envelope: &Envelope, message: &str) {
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
        let payload = encode_error_payload(ErrorCode::Acl.wire(), message);
        match build_message("kurogane_event", &reply, &payload) {
            Some(mut msg) => frame.send_process_message(ProcessId::RENDERER, Some(&mut msg)),
            None => debug!("[Event Browser] failed to build refusal message"),
        }
    }

    /// Broadcast an event to all subscribers of a given event name.
    ///
    /// Each subscription gets its own message, addressed by its id, so the
    /// renderer runs exactly the callback of that subscription (which the ACL
    /// admitted) and nothing else in the process. A subscription whose frame
    /// now shows another document receives nothing.
    pub fn broadcast(&self, cmd: &str, data: &[u8]) {
        let Some(payload) = encode_cmd_payload(cmd, data) else {
            debug!("[Event Browser] event name exceeds the protocol length limit");
            return;
        };

        let subs = self.subscriptions.lock().unwrap();
        let Some(entries) = subs.get(cmd) else {
            return;
        };

        let subs = self.subscriptions.lock().unwrap();
        if let Some(entries) = subs.get(cmd) {
            for sub in entries {
                let frame = sub.frame.clone();
                if frame.is_valid() != 0 {
                    frame.send_process_message(ProcessId::RENDERER, Some(&mut msg));
                }
            }
        }
    }
}
