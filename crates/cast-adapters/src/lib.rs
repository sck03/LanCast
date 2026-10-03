//! Network and persistence adapters. UI and platform media types stay outside.
pub mod auth;
pub use cast_domain::capabilities;
pub mod discovery;
pub mod dlna;
pub mod http_server;
#[cfg(feature = "legacy")]
pub mod legacy_bridge;
#[cfg(feature = "sender")]
pub mod live;
pub mod media_http;
pub mod protocol;
pub mod runtime;
pub mod session;
#[cfg(feature = "sender")]
pub mod ts;
