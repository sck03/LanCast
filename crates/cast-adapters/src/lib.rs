//! Network and persistence adapters. UI and platform media types stay outside.
pub mod auth;
#[cfg(feature = "file-sender")]
mod blocking_io;
pub use cast_domain::capabilities;
mod control;
pub mod discovery;
#[cfg(feature = "dlna")]
pub mod dlna;
mod events;
#[cfg(feature = "file-proxy")]
pub mod file_proxy;
#[cfg(any(feature = "dlna", feature = "file-proxy"))]
mod http_client;
#[cfg(feature = "file-sender")]
mod http_range;
#[cfg(feature = "http-server")]
pub mod http_server;
#[cfg(feature = "dlna-live")]
pub mod live;
#[cfg(feature = "dlna-live")]
pub mod live_session;
#[cfg(feature = "file-sender")]
pub mod media_http;
#[cfg(any(feature = "dlna", feature = "file-proxy"))]
mod network;
#[cfg(feature = "dlna-live")]
pub mod profiles;
pub mod protocol;
mod rtc_recovery;
pub mod runtime;
pub mod session;
#[cfg(feature = "dlna-live")]
pub mod ts;
#[cfg(feature = "http-server")]
mod write_timeout;
