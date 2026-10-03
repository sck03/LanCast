use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate: u32,
}
impl Profile {
    pub fn conservative(legacy: bool) -> Self {
        if legacy {
            Self {
                width: 1280,
                height: 720,
                fps: 30,
                bitrate: 3_000_000,
            }
        } else {
            Self {
                width: 1920,
                height: 1080,
                fps: 30,
                bitrate: 6_000_000,
            }
        }
    }
}
#[derive(Default)]
pub struct QualityController {
    bad: u32,
    good: u32,
    level: usize,
}
impl QualityController {
    /// One measured sample per second. Returns a change request, never claims it was applied.
    pub fn sample(
        &mut self,
        fps: f64,
        target: f64,
        queued_ms: f64,
        bandwidth_ratio: f64,
    ) -> Option<Profile> {
        if !fps.is_finite() || !queued_ms.is_finite() || !bandwidth_ratio.is_finite() {
            return None;
        }
        if fps < target * 0.9 || queued_ms > 150.0 {
            self.bad += 1;
            self.good = 0;
        } else {
            self.bad = 0;
            if bandwidth_ratio >= 1.5 {
                self.good += 1;
            } else {
                self.good = 0;
            }
        }
        if self.bad >= 5 && self.level > 0 {
            self.level -= 1;
            self.bad = 0;
            return Some(Profile::conservative(self.level == 0));
        }
        if self.good >= 30 && self.level == 0 {
            self.level = 1;
            self.good = 0;
            return Some(Profile::conservative(false));
        }
        None
    }
}
pub fn route(kind: &str, verified: bool, mirror: bool) -> &'static str {
    match (kind, verified, mirror) {
        ("lancast_receiver", true, _) => "lancast",
        ("dlna_renderer", true, false) => "dlna",
        ("system_only", _, _) => "system_guide",
        _ => "unsupported",
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dlna_never_mirrors() {
        assert_eq!(route("dlna_renderer", true, true), "unsupported");
        assert_eq!(route("dlna_renderer", true, false), "dlna");
        assert_eq!(route("lancast_receiver", false, true), "unsupported");
    }
}
