//! Binds the pure recovery policy to authenticated sessions and negotiation IDs.
use crate::protocol::Message;
use cast_core::recovery::{Action, Recovery};
use serde_json::json;
use uuid::Uuid;

pub const CAPABILITY: &str = "replace-v1";

pub struct RtcRecovery {
    session: Uuid,
    negotiation: Option<Uuid>,
    awaiting_offer: bool,
    policy: Recovery,
}

impl RtcRecovery {
    pub fn accepts(&self, message: &Message) -> bool {
        !self.awaiting_offer
            && message.session_id == Some(self.session)
            && self.negotiation.is_some()
            && message.body["negotiationId"]
                .as_str()
                .and_then(|s| Uuid::parse_str(s).ok())
                == self.negotiation
    }
    pub fn new(session: Uuid, now_ms: u64) -> Self {
        Self {
            session,
            negotiation: None,
            awaiting_offer: false,
            policy: Recovery::new(now_ms),
        }
    }

    pub fn offer(&mut self, message: &Message) {
        if message.session_id != Some(self.session) {
            return;
        }
        let Some(id) = message.body["negotiationId"]
            .as_str()
            .and_then(|s| Uuid::parse_str(s).ok())
        else {
            return;
        };
        let previous = match message.body.get("previousNegotiationId") {
            None => None,
            Some(value) => match value.as_str().and_then(|s| Uuid::parse_str(s).ok()) {
                Some(id) => Some(id),
                None => return,
            },
        };
        // The receiver enforces the same parent chain. Replayed offers must
        // never reopen reports from a transport already marked for replacement.
        if Some(id) == self.negotiation || previous != self.negotiation {
            return;
        }
        self.negotiation = Some(id);
        self.awaiting_offer = false;
    }

    pub fn report(&mut self, message: &Message, remote: bool, now_ms: u64) {
        if self.accepts(message) {
            self.policy
                .report(remote, message.body["state"] == "connected", now_ms);
        }
    }

    pub fn next_deadline(&self) -> Option<u64> {
        self.policy.next_deadline()
    }

    pub fn poll(&mut self, now_ms: u64) -> Option<Message> {
        match self.policy.poll(now_ms) {
            Action::Wait => None,
            Action::Restart { attempt } => {
                self.awaiting_offer = true;
                let mut event = Message::new(
                    "rtc.restart",
                    json!({"negotiationId":self.negotiation,"attempt":attempt}),
                );
                event.session_id = Some(self.session);
                Some(event)
            }
            Action::Stop => {
                let mut event =
                    Message::new("session.stop", json!({"reason":"RTC_RECOVERY_EXHAUSTED"}));
                event.session_id = Some(self.session);
                Some(event)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn repeated_offer_cannot_reactivate_the_transport_awaiting_replacement() {
        let session = Uuid::new_v4();
        let negotiation = Uuid::new_v4();
        let mut recovery = RtcRecovery::new(session, 0);
        let mut offer = Message::new("rtc.offer", json!({"negotiationId":negotiation}));
        offer.session_id = Some(session);
        recovery.offer(&offer);
        let mut state = offer.clone();
        state.kind = "rtc.state".into();
        state.body["state"] = json!("failed");
        recovery.report(&state, false, 0);
        assert_eq!(recovery.poll(1000).unwrap().kind, "rtc.restart");

        recovery.offer(&offer);
        state.body["state"] = json!("connected");
        recovery.report(&state, false, 1001);
        recovery.report(&state, true, 1001);
        assert!(!recovery.accepts(&state));
        assert_eq!(recovery.poll(5000).unwrap().body["attempt"], 2);
        assert_eq!(recovery.poll(15000).unwrap().kind, "session.stop");
    }

    #[test]
    fn invalid_offer_cannot_replace_the_current_negotiation() {
        let session = Uuid::new_v4();
        let first = Uuid::new_v4();
        let next = Uuid::new_v4();
        let mut recovery = RtcRecovery::new(session, 0);
        let mut offer = Message::new("rtc.offer", json!({"negotiationId":first}));
        offer.session_id = Some(session);
        recovery.offer(&offer);
        for body in [
            json!({"negotiationId":next}),
            json!({"negotiationId":next,"previousNegotiationId":Uuid::new_v4()}),
            json!({"negotiationId":"invalid","previousNegotiationId":first}),
            json!({"negotiationId":next,"previousNegotiationId":null}),
        ] {
            let mut invalid = offer.clone();
            invalid.body = body;
            recovery.offer(&invalid);
            assert!(recovery.accepts(&offer));
        }
        let mut replacement = offer.clone();
        replacement.body = json!({"negotiationId":next,"previousNegotiationId":first});
        recovery.offer(&replacement);
        assert!(recovery.accepts(&replacement));
        assert!(!recovery.accepts(&offer));
    }

    #[test]
    fn old_transport_and_other_session_cannot_cancel_recovery() {
        let session = Uuid::new_v4();
        let first = Uuid::new_v4();
        let second = Uuid::new_v4();
        let mut r = RtcRecovery::new(session, 0);
        let mut m = Message::new("rtc.offer", json!({"negotiationId":first}));
        m.session_id = Some(session);
        r.offer(&m);
        m.kind = "rtc.state".into();
        m.body["state"] = json!("failed");
        r.report(&m, false, 0);
        assert_eq!(r.poll(1000).unwrap().kind, "rtc.restart");
        m.body["state"] = json!("connected");
        r.report(&m, false, 1001);
        r.report(&m, true, 1001);
        let mut offer = m.clone();
        offer.body["negotiationId"] = json!(second);
        offer.body["previousNegotiationId"] = json!(first);
        r.offer(&offer);
        r.report(&m, false, 1100);
        r.report(&m, true, 1100);
        offer.session_id = Some(Uuid::new_v4());
        r.report(&offer, false, 1200);
        r.report(&offer, true, 1200);
        assert_eq!(r.poll(5000).unwrap().body["attempt"], 2);
        assert_eq!(r.poll(15000).unwrap().kind, "session.stop");
    }
}
