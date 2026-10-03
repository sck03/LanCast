//! I/O-free product types. Advertised capabilities are not measured evidence or consent.
pub mod capabilities;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Evidence {
    #[default]
    Unknown,
    Passed,
    Failed,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Intent {
    Mirror,
    File,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Route {
    WebrtcMirror,
    DlnaLive,
    ReceiverFile,
    DlnaFile,
    SystemGuide,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Use,
    Probe,
    Unavailable,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeviceCapabilities {
    pub receiver: bool,
    pub rtc: Evidence,
    pub receiver_file: Evidence,
    pub dlna: bool,
    pub dlna_file: Evidence,
    pub dlna_live: Evidence,
    pub system_guide: bool,
}
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Backends {
    pub rtc: bool,
    pub ts: bool,
    pub file: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoutePlan {
    pub route: Option<Route>,
    pub decision: Decision,
    pub reason: &'static str,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Idle,
    Preparing,
    Authorizing,
    Probing,
    Starting,
    Active,
    Stopping,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Busy,
    InvalidState,
    StaleGeneration,
    ConsentRequired,
    Unsupported,
    MediaFailed,
}

/// The platform owns the grant and releases it when stop is called. A grant is session-local.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Grant {
    pub generation: u64,
    pub platform_handle: u64,
}

pub trait MediaSessionPort {
    fn start(&mut self, route: Route, grant: Option<Grant>) -> Result<(), Error>;
    fn stop(&mut self);
}
