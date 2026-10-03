//! Pure use cases. Adapters implement ports; the core never calls OS or network APIs.
use cast_domain::*;
pub mod recovery;

pub fn plan(intent: Intent, device: &DeviceCapabilities, backend: Backends) -> RoutePlan {
    let candidates: &[(Route, bool, Evidence)] = match intent {
        Intent::Mirror => &[
            (
                Route::WebrtcMirror,
                device.receiver && backend.rtc,
                device.rtc,
            ),
            (Route::DlnaLive, device.dlna && backend.ts, device.dlna_live),
        ],
        Intent::File => &[
            (
                Route::ReceiverFile,
                device.receiver && backend.file,
                device.receiver_file,
            ),
            (
                Route::DlnaFile,
                device.dlna && backend.file,
                device.dlna_file,
            ),
        ],
    };
    for &(route, available, evidence) in candidates {
        if available && evidence != Evidence::Failed {
            return RoutePlan {
                route: Some(route),
                decision: if evidence == Evidence::Passed {
                    Decision::Use
                } else {
                    Decision::Probe
                },
                reason: if evidence == Evidence::Passed {
                    "verified_route"
                } else {
                    "explicit_probe_required"
                },
            };
        }
    }
    RoutePlan {
        route: device.system_guide.then_some(Route::SystemGuide),
        decision: Decision::Unavailable,
        reason: if device.system_guide {
            "use_system_entry"
        } else {
            "no_common_backend"
        },
    }
}

pub struct Coordinator<M: MediaSessionPort> {
    media: M,
    phase: Phase,
    generation: u64,
    route: Option<Route>,
}
impl<M: MediaSessionPort> Coordinator<M> {
    pub fn new(media: M) -> Self {
        Self {
            media,
            phase: Phase::Idle,
            generation: 0,
            route: None,
        }
    }
    pub fn phase(&self) -> Phase {
        self.phase
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    pub fn prepare(&mut self, plan: &RoutePlan) -> Result<u64, Error> {
        if self.phase != Phase::Idle {
            return Err(Error::Busy);
        }
        if plan.decision == Decision::Unavailable {
            return Err(Error::Unsupported);
        }
        self.generation = self.generation.checked_add(1).ok_or(Error::InvalidState)?;
        self.route = plan.route;
        self.phase = if plan.decision == Decision::Probe {
            Phase::Probing
        } else {
            Phase::Authorizing
        };
        Ok(self.generation)
    }
    /// Only a completed, user-triggered synthetic test promotes the pending plan.
    pub fn probed(&mut self, generation: u64, passed: bool) -> Result<(), Error> {
        self.check(generation)?;
        if self.phase != Phase::Probing {
            return Err(Error::InvalidState);
        }
        if !passed {
            self.stop();
            return Err(Error::Unsupported);
        }
        self.phase = Phase::Authorizing;
        Ok(())
    }
    pub fn start(&mut self, generation: u64, grant: Option<Grant>) -> Result<(), Error> {
        self.check(generation)?;
        if self.phase != Phase::Authorizing {
            return Err(Error::InvalidState);
        }
        let route = self.route.ok_or(Error::InvalidState)?;
        if matches!(route, Route::WebrtcMirror | Route::DlnaLive)
            && !grant.is_some_and(|g| g.generation == generation && g.platform_handle != 0)
        {
            return Err(Error::ConsentRequired);
        }
        self.phase = Phase::Starting;
        if let Err(e) = self.media.start(route, grant) {
            self.stop();
            return Err(e);
        }
        Ok(())
    }
    /// A successful start request is not evidence of a displayed frame.
    pub fn ready(&mut self, generation: u64) -> Result<(), Error> {
        self.check(generation)?;
        if self.phase != Phase::Starting {
            return Err(Error::InvalidState);
        }
        self.phase = Phase::Active;
        Ok(())
    }
    pub fn stop(&mut self) {
        if self.phase == Phase::Idle {
            return;
        }
        self.phase = Phase::Stopping;
        self.media.stop();
        self.route = None;
        self.phase = Phase::Idle;
    }
    fn check(&self, generation: u64) -> Result<(), Error> {
        if self.generation != generation || self.phase == Phase::Idle {
            Err(Error::StaleGeneration)
        } else {
            Ok(())
        }
    }
}
impl<M: MediaSessionPort> Drop for Coordinator<M> {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn route_evidence_is_independent() {
        let d = DeviceCapabilities {
            dlna: true,
            dlna_file: Evidence::Passed,
            ..Default::default()
        };
        let b = Backends {
            ts: true,
            file: true,
            rtc: false,
        };
        assert_eq!(plan(Intent::Mirror, &d, b).decision, Decision::Probe);
        assert_eq!(plan(Intent::File, &d, b).decision, Decision::Use);
        assert_eq!(
            plan(Intent::Mirror, &d, Backends::default()).decision,
            Decision::Unavailable
        );
        let d = DeviceCapabilities {
            dlna_live: Evidence::Failed,
            ..d
        };
        assert_eq!(plan(Intent::Mirror, &d, b).decision, Decision::Unavailable);
    }
    #[derive(Default)]
    struct Media {
        starts: usize,
        stops: usize,
        fail: bool,
    }
    impl MediaSessionPort for Media {
        fn start(&mut self, _: Route, _: Option<Grant>) -> Result<(), Error> {
            self.starts += 1;
            if self.fail {
                Err(Error::MediaFailed)
            } else {
                Ok(())
            }
        }
        fn stop(&mut self) {
            self.stops += 1;
        }
    }
    fn verified() -> RoutePlan {
        RoutePlan {
            route: Some(Route::WebrtcMirror),
            decision: Decision::Use,
            reason: "test",
        }
    }
    #[test]
    fn consent_stale_callbacks_and_idempotent_stop() {
        let mut c = Coordinator::new(Media::default());
        let g = c.prepare(&verified()).unwrap();
        assert_eq!(c.start(g, None), Err(Error::ConsentRequired));
        assert_eq!(c.media.starts, 0);
        c.start(
            g,
            Some(Grant {
                generation: g,
                platform_handle: 1,
            }),
        )
        .unwrap();
        assert_eq!(c.phase(), Phase::Starting);
        c.ready(g).unwrap();
        c.stop();
        c.stop();
        assert_eq!(c.media.stops, 1);
        let next = c.prepare(&verified()).unwrap();
        assert_ne!(g, next);
        assert_eq!(c.ready(g), Err(Error::StaleGeneration));
    }
    #[test]
    fn partial_initialization_is_released() {
        let mut c = Coordinator::new(Media {
            fail: true,
            ..Default::default()
        });
        let g = c.prepare(&verified()).unwrap();
        assert_eq!(
            c.start(
                g,
                Some(Grant {
                    generation: g,
                    platform_handle: 2
                })
            ),
            Err(Error::MediaFailed)
        );
        assert_eq!(c.phase(), Phase::Idle);
        assert_eq!(c.media.stops, 1);
    }
}
