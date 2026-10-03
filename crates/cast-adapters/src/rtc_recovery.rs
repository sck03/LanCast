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
        if message.session_id == Some(self.session) {
            self.negotiation = message.body["negotiationId"]
                .as_str()
                .and_then(|s| Uuid::parse_str(s).ok());
            self.awaiting_offer = false;
        }
    }

    pub fn report(&mut self, message: &Message, remote: bool, now_ms: u64) {
        let id = message.body["negotiationId"]
            .as_str()
            .and_then(|s| Uuid::parse_str(s).ok());
        if !self.awaiting_offer
            && self.negotiation.is_some()
            && id == self.negotiation
            && message.session_id == Some(self.session)
        {
            self.policy
                .report(remote, message.body["state"] == "connected", now_ms);
        }
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
