//! LanCast control plane. No captured pixels or PCM cross this boundary.
#[cfg(target_os = "android")]
mod android;
pub mod auth;
pub mod capabilities;
pub mod discovery;
pub mod dlna;
pub mod ffi;
pub mod media_http;
pub mod protocol;
pub mod runtime;
pub mod session;
