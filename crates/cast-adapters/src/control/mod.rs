//! Control transport adapters, kept separate from command composition.
mod client;
mod server;
mod wire;
pub(crate) use client::Client;
pub(crate) use server::Server;
