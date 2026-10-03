//! LanCast control plane. No captured pixels or PCM cross this boundary.
#[cfg(target_os = "android")]
mod android;
pub mod ffi;
pub use cast_adapters::{protocol, runtime};
