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
    Reconnecting,
    Stopped,
}

pub struct Session {
    pub id: Uuid,
    pub owner: Uuid,
    pub state: State,
    pub mode: String,
    pub negotiation: Option<Uuid>,
    disconnected: Option<Instant>,
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
            disconnected: None,
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
            matches!(self.state, State::Negotiating | State::Loading),
            "INVALID_STATE"
        );
        self.state = if self.mode == "mirror" {
            State::Streaming
        } else {
            State::Playing
        };
        Ok(())
    }
    pub fn disconnect(&mut self, now: Instant) {
        if self.state != State::Stopped && self.disconnected.is_none() {
            self.disconnected = Some(now);
            self.state = State::Reconnecting;
        }
    }
    pub fn resume(&mut self, now: Instant) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.state == State::Reconnecting && !self.expired(now),
            "SESSION_EXPIRED"
        );
        self.disconnected = None;
        self.negotiation = None;
        self.state = if self.mode == "mirror" {
            State::Negotiating
        } else {
            State::Loading
        };
        Ok(())
    }
    pub fn expired(&self, now: Instant) -> bool {
        self.disconnected
            .is_some_and(|t| now.saturating_duration_since(t) >= Duration::from_secs(15))
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

pub struct RetryBudget {
    started: Instant,
    attempts: usize,
}
impl RetryBudget {
    pub fn new(now: Instant) -> Self {
        Self {
            started: now,
            attempts: 0,
        }
    }
    pub fn next_delay(&mut self, now: Instant) -> Option<Duration> {
        let delay = Duration::from_secs(*[1, 2, 4].get(self.attempts)?);
        if now.duration_since(self.started) + delay >= Duration::from_secs(15) {
            return None;
        }
        self.attempts += 1;
        Some(delay)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lifecycle_and_owner() {
        let user = Uuid::new_v4();
        let mut s = Session::new(user, "mirror").unwrap();
        assert!(s.authorize(Uuid::new_v4(), Some(s.id)).is_err());
        s.ready().unwrap();
        let now = Instant::now();
        s.disconnect(now);
        // Further failures must not extend the original deadline.
        s.disconnect(now + Duration::from_secs(14));
        assert!(s.resume(now + Duration::from_secs(15)).is_err());
        s.stop();
        s.stop();
        assert!(s.ready().is_err());
    }
    #[test]
    fn retries_are_bounded() {
        let now = Instant::now();
        let mut r = RetryBudget::new(now);
        for n in [1, 2, 4] {
            assert_eq!(r.next_delay(now), Some(Duration::from_secs(n)));
        }
        assert_eq!(r.next_delay(now), None);
    }
}
