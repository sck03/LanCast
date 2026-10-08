//! Optional GPL AirPlay implementation. Encoded media never enters cast-core or control JSON.
#[cfg(target_os = "android")]
mod android;
pub mod avc;
pub mod engine;
pub mod identity;
