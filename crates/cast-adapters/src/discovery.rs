mod mdns;
mod results;
#[cfg(feature = "dlna")]
mod ssdp;
mod tasks;
pub use mdns::{Advertisement, scan_lancast};
#[cfg(feature = "dlna")]
pub use ssdp::{scan_dlna, ssdp_location};
#[cfg(feature = "dlna")]
pub(crate) use tasks::DlnaDiscovery;
pub(crate) use tasks::ScanTask;

const SCAN_WINDOW: std::time::Duration = std::time::Duration::from_secs(4);
