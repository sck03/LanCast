//! Sender session correlation and RTC signals, independent of sockets and host delivery.
use crate::{
    protocol::Message,
    rtc_recovery::{CAPABILITY, RtcRecovery},
};
use anyhow::Context;
use serde_json::json;
use uuid::Uuid;

#[derive(Default)]
pub(super) struct ClientSession {
    active: Option<Uuid>,
    pending: Option<(Uuid, bool)>,
    recovery: Option<RtcRecovery>,
}

pub(super) enum Outgoing {
    Forward(Message),
    Stop(Message),
    Ignore,
}

impl ClientSession {
    pub(super) fn outgoing(&mut self, request: Message, now: u64) -> Outgoing {
        if request.kind == "session.start" {
            self.pending = Some((
                request.id,
                request.body["mode"] == "mirror" && request.body["rtcRecovery"] == CAPABILITY,
            ));
        } else if matches!(request.kind.as_str(), "ping" | "pong") {
            // Heartbeats belong to the connection, not a media session.
        } else {
            if self.active.is_none() || request.session_id != self.active {
                return Outgoing::Ignore;
            }
            match request.kind.as_str() {
                "rtc.state" => {
                    if let Some(recovery) = &mut self.recovery {
                        recovery.report(&request, false, now);
                    } else if request.body["state"] != "connected" {
                        let stop =
                            request.reply("session.stop", json!({"reason":"RTC_CONNECTION_LOST"}));
                        self.clear();
                        return Outgoing::Stop(stop);
                    }
                    return Outgoing::Ignore;
                }
                "rtc.offer" if self.recovery.as_mut().is_some_and(|r| !r.offer(&request)) => {
                    return Outgoing::Ignore;
                }
                "rtc.ice" if self.recovery.as_ref().is_some_and(|r| !r.accepts(&request)) => {
                    return Outgoing::Ignore;
                }
                "session.stop" => self.clear(),
                _ => {}
            }
        }
        Outgoing::Forward(request)
    }

    pub(super) fn incoming(&mut self, response: &Message, now: u64) -> anyhow::Result<bool> {
        if response.kind == "session.accepted" {
            let Some((request, recover)) = self.pending else {
                return Ok(false);
            };
            if response.reply_to != Some(request) {
                return Ok(false);
            }
            let session = response.session_id.context("SESSION_REQUIRED")?;
            self.active = Some(session);
            self.pending = None;
            self.recovery = (recover && response.body["rtcRecovery"] == CAPABILITY)
                .then(|| RtcRecovery::new(session, now));
            return Ok(true);
        }
        if let Some(session) = response.session_id {
            if self.active != Some(session) {
                return Ok(false);
            }
        } else if response.kind == "error" {
            if let Some(reply) = response.reply_to {
                if self.pending.is_none_or(|(request, _)| request != reply) {
                    return Ok(false);
                }
                // A rejected start never alters the already active session.
                self.pending = None;
                return Ok(true);
            }
        } else {
            // Session signals always carry an ID; unscoped acknowledgements are harmless.
            return Ok(response.kind == "ack");
        }
        match response.kind.as_str() {
            "rtc.state" => {
                if let Some(recovery) = &mut self.recovery {
                    recovery.report(response, true, now);
                }
                return Ok(false);
            }
            "rtc.answer" | "rtc.ice"
                if self.recovery.as_ref().is_some_and(|r| !r.accepts(response)) =>
            {
                return Ok(false);
            }
            "session.stop" | "error" => self.clear(),
            _ => {}
        }
        Ok(true)
    }

    pub(super) fn poll(&mut self, now: u64) -> Option<Message> {
        let message = self.recovery.as_mut()?.poll(now)?;
        if message.kind == "session.stop" {
            self.clear();
        }
        Some(message)
    }

    pub(super) fn next_deadline(&self) -> Option<u64> {
        self.recovery.as_ref().and_then(RtcRecovery::next_deadline)
    }

    fn clear(&mut self) {
        self.active = None;
        self.pending = None;
        self.recovery = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acceptance_and_errors_must_match_the_latest_start() {
        let mut session = ClientSession::default();
        let first = Message::new("session.start", json!({"mode":"file"}));
        let next = Message::new(
            "session.start",
            json!({"mode":"mirror","rtcRecovery":CAPABILITY}),
        );
        session.outgoing(first.clone(), 0);
        session.outgoing(next.clone(), 0);
        let mut accepted = first.reply("session.accepted", json!({"rtcRecovery":CAPABILITY}));
        accepted.session_id = Some(Uuid::new_v4());
        assert!(!session.incoming(&accepted, 1).unwrap());
        assert!(!session.incoming(&first.error("BUSY"), 1).unwrap());
        accepted.reply_to = Some(next.id);
        assert!(session.incoming(&accepted, 2).unwrap());
        assert!(!session.incoming(&accepted, 3).unwrap());
        assert_eq!(session.next_deadline(), Some(20_002));

        session.outgoing(first.clone(), 4);
        assert!(session.incoming(&first.error("BUSY"), 5).unwrap());
        assert_eq!(session.active, accepted.session_id);
        assert_eq!(session.next_deadline(), Some(20_002));
        assert!(!session.incoming(&first.error("BUSY"), 6).unwrap());
    }

    #[test]
    fn rejected_or_malformed_start_cannot_create_a_session() {
        let mut session = ClientSession::default();
        let start = Message::new("session.start", json!({"mode":"file"}));
        session.outgoing(start.clone(), 0);
        let mut accepted = start.reply("session.accepted", json!({}));
        assert!(session.incoming(&accepted, 1).is_err());
        assert!(session.active.is_none());
        assert!(session.incoming(&start.error("BUSY"), 2).unwrap());
        accepted.session_id = Some(Uuid::new_v4());
        assert!(!session.incoming(&accepted, 3).unwrap());
        assert!(session.active.is_none());
        assert!(session.next_deadline().is_none());
    }

    #[test]
    fn current_stop_retires_signals_until_a_new_correlated_acceptance() {
        for remote in [false, true] {
            let mut session = ClientSession::default();
            let start = Message::new(
                "session.start",
                json!({"mode":"mirror","rtcRecovery":CAPABILITY}),
            );
            session.outgoing(start.clone(), 0);
            let mut accepted = start.reply("session.accepted", json!({"rtcRecovery":CAPABILITY}));
            accepted.session_id = Some(Uuid::new_v4());
            assert!(session.incoming(&accepted, 1).unwrap());
            let mut stop = Message::new("session.stop", json!({}));
            stop.session_id = accepted.session_id;
            if remote {
                assert!(session.incoming(&stop, 2).unwrap());
            } else {
                assert!(matches!(
                    session.outgoing(stop.clone(), 2),
                    Outgoing::Forward(_)
                ));
            }
            assert!(session.next_deadline().is_none());
            assert!(!session.incoming(&stop, 3).unwrap());
            assert!(matches!(
                session.outgoing(stop.clone(), 3),
                Outgoing::Ignore
            ));
            let mut late = stop;
            late.kind = "rtc.state".into();
            late.body = json!({"state":"failed","negotiationId":Uuid::new_v4()});
            assert!(matches!(session.outgoing(late, 4), Outgoing::Ignore));

            let next = Message::new("session.start", json!({"mode":"file"}));
            session.outgoing(next.clone(), 5);
            let mut accepted = next.reply("session.accepted", json!({}));
            accepted.session_id = Some(Uuid::new_v4());
            assert!(session.incoming(&accepted, 6).unwrap());
            assert_eq!(session.active, accepted.session_id);
        }
    }
}
