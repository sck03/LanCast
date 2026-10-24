use crate::protocol::Message;
use serde::Serialize;
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Negotiating,
    Loading,
    Streaming,
    Playing,
    Stopped,
}

pub struct Session {
    pub id: Uuid,
    pub owner: Uuid,
    pub state: State,
    pub mode: String,
    pub negotiation: Option<Uuid>,
    pub rtc_recovery: bool,
    cache: VecDeque<(Instant, Uuid, Message)>,
}
impl Session {
    pub fn new(owner: Uuid, mode: &str) -> anyhow::Result<Self> {
        anyhow::ensure!(matches!(mode, "mirror" | "file"), "CAPABILITY_UNSUPPORTED");
        Ok(Self {
            id: Uuid::new_v4(),
            owner,
            state: if mode == "mirror" {
                State::Negotiating
            } else {
                State::Loading
            },
            mode: mode.into(),
            negotiation: None,
            rtc_recovery: false,
            cache: VecDeque::new(),
        })
    }
    pub fn authorize(&self, owner: Uuid, id: Option<Uuid>) -> anyhow::Result<()> {
        anyhow::ensure!(self.owner == owner && id == Some(self.id), "AUTH_REQUIRED");
        anyhow::ensure!(self.state != State::Stopped, "SESSION_EXPIRED");
        Ok(())
    }
    pub fn ready(&mut self) -> anyhow::Result<()> {
        anyhow::ensure!(
            matches!(
                self.state,
                State::Negotiating | State::Loading | State::Streaming | State::Playing
            ),
            "INVALID_STATE"
        );
        self.state = if self.mode == "mirror" {
            State::Streaming
        } else {
            State::Playing
        };
        Ok(())
    }
    pub fn negotiate(&mut self, id: Uuid, previous: Option<Uuid>) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.mode == "mirror" && matches!(self.state, State::Negotiating | State::Streaming),
            "INVALID_STATE"
        );
        anyhow::ensure!(Some(id) != self.negotiation, "STALE_NEGOTIATION");
        anyhow::ensure!(previous == self.negotiation, "STALE_NEGOTIATION");
        anyhow::ensure!(
            self.negotiation.is_none() || self.rtc_recovery,
            "RECOVERY_NOT_NEGOTIATED"
        );
        self.negotiation = Some(id);
        self.state = State::Negotiating;
        Ok(())
    }
    pub fn stop(&mut self) {
        self.state = State::Stopped;
        self.cache.clear();
    }
    pub fn remember(&mut self, request: Uuid, response: Message, now: Instant) {
        self.cache.push_back((now, request, response));
        while self.cache.len() > 256
            || self
                .cache
                .front()
                .is_some_and(|(t, _, _)| now.duration_since(*t) > Duration::from_secs(60))
        {
            self.cache.pop_front();
        }
    }
    pub fn cached(&self, request: Uuid, now: Instant) -> Option<Message> {
        self.cache
            .iter()
            .find(|(t, id, _)| *id == request && now.duration_since(*t) <= Duration::from_secs(60))
            .map(|(_, _, r)| r.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn replacement_requires_capability_and_current_parent() {
        let mut s = Session::new(Uuid::new_v4(), "mirror").unwrap();
        let first = Uuid::new_v4();
        let next = Uuid::new_v4();
        s.negotiate(first, None).unwrap();
        assert!(s.negotiate(next, Some(first)).is_err());
        s.rtc_recovery = true;
        assert!(s.negotiate(first, Some(first)).is_err());
        assert!(s.negotiate(next, None).is_err());
        s.negotiate(next, Some(first)).unwrap();
        assert!(s.negotiate(first, None).is_err());
        assert_eq!(s.negotiation, Some(next));
        s.stop();
        assert!(s.negotiate(Uuid::new_v4(), Some(next)).is_err());
    }
    #[test]
    fn lifecycle_and_owner() {
        let user = Uuid::new_v4();
        let mut s = Session::new(user, "mirror").unwrap();
        assert!(s.authorize(Uuid::new_v4(), Some(s.id)).is_err());
        s.ready().unwrap();
        s.stop();
        s.stop();
        assert!(s.ready().is_err());
    }
}
