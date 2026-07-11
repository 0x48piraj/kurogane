//! Browser-side event dispatch.
//!
//! Handles subscription management for renderer processes and delivers emitted
//! events to subscribed frames.

use cef::*;

use crate::debug;
use crate::ipc::browser_state::IpcContext;
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

        let persistent = (envelope.flags & FLAG_PERSISTENT) != 0;
        let browser_id = match ctx.browser_id {
            Some(id) => id,
            None => {
                debug!("[Event Browser] subscribe without browser_id");
                return false;
            }
        };

        let mut subs = self.subscriptions.lock().unwrap();
        subs.entry(event_name.to_string())
            .or_default()
            .push(crate::ipc::event::EventSubscription {
                id: envelope.correlation_id,
                frame: frame.clone(),
                browser_id,
                persistent,
            });

        debug!(
            "[Event Browser] subscribed '{}' browser={} persistent={}",
            event_name,
            browser_id.as_u32(),
            persistent
        );
        true
    }

    fn on_unsubscribe(
        &self,
        envelope: &Envelope,
        payload: &[u8],
        ctx: IpcContext,
    ) -> bool {
        let (event_name, _rest) = match decode_cmd_payload(payload) {
            Some(v) => v,
            None => {
                debug!("[Event Browser] invalid unsubscribe payload");
                return false;
            }
        };

        if let Some(browser_id) = ctx.browser_id {
            self.remove_subscription(event_name, browser_id, envelope.correlation_id);
            debug!(
                "[Event Browser] unsubscribed '{}' browser={} id={}",
                event_name,
                browser_id.as_u32(),
                envelope.correlation_id,
            );
        }
        true
    }

    /// Broadcast an event to all subscribers of a given event name.
    pub fn broadcast(&self, cmd: &str, data: &[u8]) {
        let envelope = Envelope {
            version: ENVELOPE_VERSION,
            subsystem: SUB_EVENT,
            opcode: EVENT_EMIT,
            flags: 0,
            correlation_id: 0,
            payload_kind: PAYLOAD_JSON,
        };

        let payload = encode_cmd_payload(cmd, data);
        let Some(mut msg) = build_message("kurogane_event", &envelope, &payload) else {
            debug!("[Event Browser] failed to build broadcast message");
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
