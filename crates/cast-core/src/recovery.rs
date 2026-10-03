//! Clock-injected RTC recovery policy. No signaling, platform, or timer dependencies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Wait,
    Restart { attempt: u8 },
    Stop,
}

/// One initial connection deadline and one fixed deadline per outage. Repeated
/// failures never extend it. Only both endpoints on the current transport recover it.
#[derive(Debug)]
pub struct Recovery {
    initial_deadline: u64,
    outage: Option<u64>,
    attempts: u8,
    connected: [bool; 2],
    stopped: bool,
}

impl Recovery {
    pub fn new(now_ms: u64) -> Self {
        Self {
            initial_deadline: now_ms.saturating_add(20_000),
            outage: None,
            attempts: 0,
            connected: [false; 2],
            stopped: false,
        }
    }

    pub fn report(&mut self, remote: bool, connected: bool, now_ms: u64) {
        if self.stopped {
            return;
        }
        self.connected[usize::from(remote)] = connected;
        if !connected && self.outage.is_none() {
            self.outage = Some(now_ms);
            self.attempts = 0;
        }
        if self.connected == [true, true] {
            self.outage = None;
            self.attempts = 0;
        }
    }

    pub fn poll(&mut self, now_ms: u64) -> Action {
        if self.stopped {
            return Action::Wait;
        }
        if let Some(started) = self.outage {
            let elapsed = now_ms.saturating_sub(started);
            if elapsed >= 15_000 {
                self.stopped = true;
                return Action::Stop;
            }
            if let Some(delay) = [1_000, 5_000, 10_000].get(usize::from(self.attempts))
                && elapsed >= *delay
            {
                self.attempts += 1;
                self.connected = [false; 2];
                return Action::Restart {
                    attempt: self.attempts,
                };
            }
        } else if self.connected != [true, true] && now_ms >= self.initial_deadline {
            self.stopped = true;
            return Action::Stop;
        }
        Action::Wait
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_failures_cannot_extend_outage_or_retry_forever() {
        let mut r = Recovery::new(0);
        r.report(false, true, 1);
        r.report(true, true, 2);
        r.report(false, false, 100);
        for (now, attempt) in [(1100, 1), (5100, 2), (10100, 3)] {
            r.report(true, false, now);
            assert_eq!(r.poll(now), Action::Restart { attempt });
            assert_eq!(r.poll(now), Action::Wait);
        }
        assert_eq!(r.poll(15100), Action::Stop);
        r.report(false, true, 15101);
        r.report(true, true, 15101);
        assert_eq!(r.poll(30000), Action::Wait);
    }

    #[test]
    fn transient_outage_recovers_but_one_sided_connection_does_not() {
        let mut r = Recovery::new(0);
        r.report(false, true, 0);
        r.report(true, true, 0);
        r.report(true, false, 10);
        r.report(true, true, 500);
        assert_eq!(r.poll(2000), Action::Wait);
        r.report(false, false, 2000);
        assert_eq!(r.poll(3000), Action::Restart { attempt: 1 });
        r.report(false, true, 3100);
        assert_eq!(r.poll(7000), Action::Restart { attempt: 2 });
        r.report(false, true, 7100);
        r.report(true, true, 7200);
        assert_eq!(r.poll(90000), Action::Wait);
    }

    #[test]
    fn missing_media_and_clock_rollback_are_bounded() {
        let mut r = Recovery::new(100);
        assert_eq!(r.poll(0), Action::Wait);
        r.report(false, true, 200);
        assert_eq!(r.poll(20100), Action::Stop);
    }
}
