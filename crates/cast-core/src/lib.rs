//! Pure route planning and RTC recovery policy. No OS or network APIs.
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
}
