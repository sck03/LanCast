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
    established: bool,
    deadline_reached: bool,
    stopped: bool,
}

impl Recovery {
    pub fn new(now_ms: u64) -> Self {
        Self {
            initial_deadline: now_ms.saturating_add(20_000),
            outage: None,
            attempts: 0,
            connected: [false; 2],
            established: false,
            deadline_reached: false,
            stopped: false,
        }
    }

    pub fn report(&mut self, remote: bool, connected: bool, now_ms: u64) {
        if self.stopped || self.deadline_reached {
            return;
        }
        // A late state report must not revive a deadline that has already
        // expired.  Check before applying the new state so the boundary is
        // deterministic even when a report races the periodic poll.
        if self.expired(now_ms) {
            self.deadline_reached = true;
            return;
        }
        self.connected[usize::from(remote)] = connected;
        if !connected && self.outage.is_none() {
            self.outage = Some(now_ms);
            self.attempts = 0;
        }
        if self.connected == [true, true] {
            self.established = true;
            self.outage = None;
            self.attempts = 0;
        }
    }

    pub fn poll(&mut self, now_ms: u64) -> Action {
        if self.stopped {
            return Action::Wait;
        }
        if self.deadline_reached || self.expired(now_ms) {
            self.stopped = true;
            return Action::Stop;
        }
        if let Some(started) = self.outage {
            let elapsed = now_ms.saturating_sub(started);
            if let Some(delay) = [1_000, 5_000, 10_000].get(usize::from(self.attempts))
                && elapsed >= *delay
            {
                self.attempts += 1;
                self.connected = [false; 2];
                return Action::Restart {
                    attempt: self.attempts,
                };
            }
        }
        Action::Wait
    }
    fn expired(&self, now_ms: u64) -> bool {
        (!self.established && now_ms >= self.initial_deadline)
            || self
                .outage
                .is_some_and(|started| now_ms.saturating_sub(started) >= 15_000)
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

    #[test]
    fn late_reports_cannot_revive_initial_or_outage_deadlines() {
        let mut initial = Recovery::new(0);
        initial.report(false, true, 19_000);
        initial.report(true, true, 20_000);
        assert_eq!(initial.poll(20_000), Action::Stop);
        initial.report(true, true, 20_001);
        assert_eq!(initial.poll(20_001), Action::Wait);

        let mut outage = Recovery::new(0);
        outage.report(false, true, 0);
        outage.report(true, true, 0);
        outage.report(false, false, 100);
        outage.report(false, true, 15_100);
        outage.report(true, true, 15_100);
        assert_eq!(outage.poll(15_100), Action::Stop);
        outage.report(false, true, 15_101);
        assert_eq!(outage.poll(20_000), Action::Wait);
    }

    #[test]
    fn first_failure_never_extends_initial_connection_deadline() {
        let mut recovery = Recovery::new(0);
        recovery.report(false, false, 19_000);
        assert_eq!(recovery.poll(19_999), Action::Wait);
        assert_eq!(recovery.poll(20_000), Action::Stop);
    }
}
