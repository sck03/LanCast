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
    pending: Option<(Uuid, bool, bool)>,
    file_session: bool,
    file_ready: bool,
    file_deadline: Option<u64>,
    recovery: Option<RtcRecovery>,
}

pub(super) enum Outgoing {
    Forward(Message),
    Stop(Message),
    Reject(Message),
    Ignore,
}

impl ClientSession {
    pub(super) fn outgoing(&mut self, request: Message, now: u64) -> Outgoing {
        if request.kind == "playback.command"
            && (!self.file_session
                || !self.file_ready
                || self.active.is_none()
                || request.session_id != self.active)
        {
            return Outgoing::Reject(request);
        }
        if request.kind == "session.start" {
            self.pending = Some((
                request.id,
                request.body["mode"] == "mirror" && request.body["rtcRecovery"] == CAPABILITY,
                request.body["mode"] == "file",
            ));
        } else if matches!(request.kind.as_str(), "ping" | "pong") {
            // Heartbeats belong to the connection, not a media session.
        } else {
            if self.active.is_none() || request.session_id != self.active {
                return Outgoing::Ignore;
            }
            match request.kind.as_str() {
                "file.load" => {
                    self.file_ready = false;
                    self.file_deadline = Some(now.saturating_add(20_000));
                }
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
            let Some((request, recover, file)) = self.pending else {
                return Ok(false);
            };
            if response.reply_to != Some(request) {
                return Ok(false);
            }
            let session = response.session_id.context("SESSION_REQUIRED")?;
            self.active = Some(session);
            self.file_session = file;
            self.file_deadline = file.then(|| now.saturating_add(20_000));
            self.file_ready = false;
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
                if self.pending.is_none_or(|(request, _, _)| request != reply) {
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
        let transport_signal = matches!(response.kind.as_str(), "rtc.answer" | "rtc.ice")
            || (response.kind == "session.state" && response.body["state"] == "ready");
        if transport_signal && self.recovery.as_ref().is_some_and(|r| !r.accepts(response)) {
            return Ok(false);
        }
        match response.kind.as_str() {
            "session.state" if self.file_session => {
                if self.file_deadline.is_some_and(|deadline| now >= deadline) {
                    return Ok(false);
                }
                self.file_ready = response.body["state"] == "ready";
                if self.file_ready {
                    self.file_deadline = None;
                }
            }
            "rtc.state" => {
                if let Some(recovery) = &mut self.recovery {
                    recovery.report(response, true, now);
                }
                return Ok(false);
            }
            "session.stop" | "error" => self.clear(),
            _ => {}
        }
        Ok(true)
    }

    pub(super) fn poll(&mut self, now: u64) -> Option<Message> {
        if self.file_deadline.is_some_and(|deadline| now >= deadline) {
            let mut stop = Message::new("session.stop", json!({"reason":"FILE_LOAD_TIMEOUT"}));
            stop.session_id = self.active;
            self.clear();
            return Some(stop);
        }
        let message = self.recovery.as_mut()?.poll(now)?;
        if message.kind == "session.stop" {
            self.clear();
        }
        Some(message)
    }

    pub(super) fn next_deadline(&self) -> Option<u64> {
        self.file_deadline
            .or_else(|| self.recovery.as_ref().and_then(RtcRecovery::next_deadline))
    }

    fn clear(&mut self) {
        self.active = None;
        self.file_session = false;
        self.file_deadline = None;
        self.file_ready = false;
        self.pending = None;
        self.recovery = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_loading_has_a_fixed_deadline_and_ready_disarms_it() {
        for completes in [false, true] {
            let mut session = ClientSession::default();
            let start = Message::new("session.start", json!({"mode":"file"}));
            session.outgoing(start.clone(), 0);
            let mut accepted = start.reply("session.accepted", json!({}));
            accepted.session_id = Some(Uuid::new_v4());
            session.incoming(&accepted, 1).unwrap();
            let mut state = Message::new("session.state", json!({"state":"loading"}));
            state.session_id = accepted.session_id;
            session.incoming(&state, 15_000).unwrap();
            assert_eq!(session.next_deadline(), Some(20_001));
            if completes {
                state.body["state"] = json!("ready");
                session.incoming(&state, 19_000).unwrap();
                assert!(session.next_deadline().is_none());
                assert!(session.poll(25_000).is_none());
            } else {
                state.body["state"] = json!("ready");
                assert!(!session.incoming(&state, 20_001).unwrap());
                assert!(session.poll(20_000).is_none());
                assert_eq!(
                    session.poll(20_001).unwrap().body["reason"],
                    "FILE_LOAD_TIMEOUT"
                );
                assert!(session.active.is_none());
            }
        }
    }

    #[test]
    fn file_controls_require_current_ready_session_and_reset_on_reload() {
        let mut session = ClientSession::default();
        let mut command = Message::new("playback.command", json!({"action":"pause"}));
        assert!(matches!(
            session.outgoing(command.clone(), 0),
            Outgoing::Reject(_)
        ));
        let start = Message::new("session.start", json!({"mode":"file"}));
        session.outgoing(start.clone(), 1);
        let mut accepted = start.reply("session.accepted", json!({}));
        accepted.session_id = Some(Uuid::new_v4());
        session.incoming(&accepted, 2).unwrap();
        command.session_id = accepted.session_id;
        assert!(matches!(
            session.outgoing(command.clone(), 3),
            Outgoing::Reject(_)
        ));
        let mut ready = Message::new("session.state", json!({"state":"ready"}));
        ready.session_id = Some(Uuid::new_v4());
        assert!(!session.incoming(&ready, 4).unwrap());
        assert!(matches!(
            session.outgoing(command.clone(), 5),
            Outgoing::Reject(_)
        ));
        ready.session_id = accepted.session_id;
        session.incoming(&ready, 6).unwrap();
        assert!(matches!(
            session.outgoing(command.clone(), 7),
            Outgoing::Forward(_)
        ));
        let mut reload = command.clone();
        reload.kind = "file.load".into();
        session.outgoing(reload, 8);
        assert!(matches!(session.outgoing(command, 9), Outgoing::Reject(_)));
    }

    #[test]
    fn ready_requires_the_current_transport_and_cannot_cross_a_restart() {
        let mut session = ClientSession::default();
        let start = Message::new(
            "session.start",
            json!({"mode":"mirror","rtcRecovery":CAPABILITY}),
        );
        session.outgoing(start.clone(), 0);
        let mut accepted = start.reply("session.accepted", json!({"rtcRecovery":CAPABILITY}));
        accepted.session_id = Some(Uuid::new_v4());
        assert!(session.incoming(&accepted, 0).unwrap());
        let mut ready = Message::new("session.state", json!({"state":"ready"}));
        ready.session_id = accepted.session_id;
        assert!(!session.incoming(&ready, 1).unwrap());

        let first = Uuid::new_v4();
        let next = Uuid::new_v4();
        let mut offer = Message::new("rtc.offer", json!({"negotiationId":first}));
        offer.session_id = accepted.session_id;
        assert!(matches!(
            session.outgoing(offer.clone(), 1),
            Outgoing::Forward(_)
        ));
        for id in [serde_json::Value::Null, json!("invalid"), json!(next)] {
            ready.body["negotiationId"] = id;
            assert!(!session.incoming(&ready, 2).unwrap());
        }
        ready.body["negotiationId"] = json!(first);
        assert!(session.incoming(&ready, 2).unwrap());
        let mut failed = offer.clone();
        failed.kind = "rtc.state".into();
        failed.body["state"] = json!("failed");
        session.outgoing(failed, 3);
        assert_eq!(session.poll(1003).unwrap().kind, "rtc.restart");
        assert!(!session.incoming(&ready, 1004).unwrap());
        offer.body = json!({"negotiationId":next,"previousNegotiationId":first});
        assert!(matches!(
            session.outgoing(offer, 1004),
            Outgoing::Forward(_)
        ));
        assert!(!session.incoming(&ready, 1005).unwrap());
        ready.body["negotiationId"] = json!(next);
        assert!(session.incoming(&ready, 1006).unwrap());
        assert_eq!(session.next_deadline(), Some(5003));
    }

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
