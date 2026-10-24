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
