//! Clock-injected RTC recovery policy. No signaling, platform, or timer dependencies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Wait,
    Restart { attempt: u8 },
    Stop,
}

const RETRY_DELAYS: [u64; 3] = [1_000, 5_000, 10_000];
const OUTAGE_TIMEOUT: u64 = 15_000;

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
        // deterministic even when a report races the deadline timer.
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
            if let Some(delay) = RETRY_DELAYS.get(usize::from(self.attempts))
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

    /// Absolute injected-clock deadline for the next action. A healthy,
    /// established transport needs no recovery timer until a failure report.
    pub fn next_deadline(&self) -> Option<u64> {
        if self.stopped {
            return None;
        }
        let initial = (!self.established).then_some(self.initial_deadline);
        let outage = self.outage.map(|started| {
            started.saturating_add(
                RETRY_DELAYS
                    .get(usize::from(self.attempts))
                    .copied()
                    .unwrap_or(OUTAGE_TIMEOUT),
            )
        });
        initial.into_iter().chain(outage).min()
    }

    fn expired(&self, now_ms: u64) -> bool {
        (!self.established && now_ms >= self.initial_deadline)
            || self
                .outage
                .is_some_and(|started| now_ms >= started.saturating_add(OUTAGE_TIMEOUT))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deadlines_follow_retry_schedule_and_disarm_after_recovery() {
        let mut r = Recovery::new(100);
        assert_eq!(r.next_deadline(), Some(20_100));
        r.report(false, true, 200);
        r.report(true, true, 300);
        assert_eq!(r.next_deadline(), None);
        r.report(false, false, 400);
        for (deadline, attempt) in [(1400, 1), (5400, 2), (10400, 3)] {
            assert_eq!(r.next_deadline(), Some(deadline));
            assert_eq!(r.poll(deadline - 1), Action::Wait);
            assert_eq!(r.poll(deadline), Action::Restart { attempt });
        }
        assert_eq!(r.next_deadline(), Some(15400));
        r.report(false, true, 11000);
        r.report(true, true, 11001);
        assert_eq!(r.next_deadline(), None);
        r.report(true, false, 30000);
        assert_eq!(r.next_deadline(), Some(31000));
        assert_eq!(r.poll(45000), Action::Stop);
        assert_eq!(r.next_deadline(), None);
    }

    #[test]
    fn initial_deadline_caps_retries_and_late_reports_still_stop() {
        let mut r = Recovery::new(0);
        r.report(false, false, 19000);
        assert_eq!(r.next_deadline(), Some(20000));
        r.report(false, true, 20000);
        r.report(true, true, 20000);
        assert_eq!(r.next_deadline(), Some(20000));
        assert_eq!(r.poll(20000), Action::Stop);
        assert_eq!(r.next_deadline(), None);
    }

    #[test]
    fn saturated_clock_deadline_can_be_reached() {
        let mut r = Recovery::new(u64::MAX - 100);
        r.report(false, true, u64::MAX - 99);
        r.report(true, true, u64::MAX - 98);
        r.report(false, false, u64::MAX - 97);
        assert_eq!(r.next_deadline(), Some(u64::MAX));
        assert_eq!(r.poll(u64::MAX), Action::Stop);
        assert_eq!(r.next_deadline(), None);
    }

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
