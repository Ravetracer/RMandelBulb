// SPDX-License-Identifier: LGPL-2.1-or-later
//! Network rendering: render clients announce themselves on the local network, a server
//! (CLI or app) sends them a job with its files, hands out frames or sample ranges and
//! collects the results.
//!
//! Ports: UDP [`DISCOVERY_PORT`] for the announcements, TCP [`CLIENT_PORT`] on the clients.

pub mod assets;
pub mod client;
pub mod discovery;
pub mod proto;
pub mod server;

/// UDP port the clients announce themselves on.
pub const DISCOVERY_PORT: u16 = 47810;
/// TCP port a client accepts the server on.
pub const CLIENT_PORT: u16 = 47811;
/// Changes whenever the messages change.
pub const PROTOCOL: u32 = 1;

/// This machine's name (shown in the client lists).
pub fn host_name() -> String {
    sysinfo::System::host_name().ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "this machine".into())
}

/// Version string clients and servers must share (same program version, same protocol).
pub fn version() -> String {
    format!("{} (protocol {PROTOCOL})", env!("CARGO_PKG_VERSION"))
}
