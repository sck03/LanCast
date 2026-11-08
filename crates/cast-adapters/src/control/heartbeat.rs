//! Shared receive deadline; sending traffic never proves the peer is alive.
use std::time::Duration;
use tokio::time::Instant;

const RECEIVE_TIMEOUT: Duration = Duration::from_secs(15);

pub(super) struct Heartbeat {
    deadline: Instant,
}

impl Heartbeat {
    pub(super) fn new() -> Self {
        Self {
            deadline: Instant::now() + RECEIVE_TIMEOUT,
        }
    }

    pub(super) fn deadline(&self) -> Instant {
        self.deadline
    }

    pub(super) fn check(&self) -> anyhow::Result<()> {
        anyhow::ensure!(Instant::now() < self.deadline, "HEARTBEAT_TIMEOUT");
        Ok(())
    }

    pub(super) fn received(&mut self) -> anyhow::Result<()> {
        let now = Instant::now();
        // A ready socket must not revive an expired connection before its timer runs.
        anyhow::ensure!(now < self.deadline, "HEARTBEAT_TIMEOUT");
        self.deadline = now + RECEIVE_TIMEOUT;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn late_receive_cannot_rearm_an_expired_deadline() {
        let mut heartbeat = Heartbeat::new();
        tokio::time::advance(Duration::from_millis(123)).await;
        heartbeat.received().unwrap();
        let deadline = heartbeat.deadline();
        tokio::time::advance(RECEIVE_TIMEOUT).await;
        assert_eq!(
            heartbeat.received().unwrap_err().to_string(),
            "HEARTBEAT_TIMEOUT"
        );
        assert_eq!(heartbeat.deadline(), deadline);
        assert!(heartbeat.check().is_err());
    }
}
