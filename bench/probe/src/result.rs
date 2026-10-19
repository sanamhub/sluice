//! The one JSON line `fetch` prints.

use anyhow::{Context, Result};
use serde::Serialize;

use crate::{
    cli::{Congestion, Mode, Sink},
    net::Windows,
};

/// A finished transfer. Fields iroh does not expose are `null`, never estimated.
#[derive(Serialize)]
pub struct Output {
    /// `raw` or `blobs`.
    pub mode: Mode,
    /// Size the ticket promised.
    pub size_bytes: u64,
    /// Payload bytes counted at the receiver. `fetch` fails unless this equals `size_bytes`.
    pub received_bytes: u64,
    /// Parallel connections used.
    pub connections: u16,
    /// Congestion controller on this end.
    pub congestion: Congestion,
    /// Windows on this end.
    pub window: Windows,
    /// `discard` or `store` in blobs mode, `null` in raw mode.
    pub sink: Option<Sink>,
    /// `disabled` or the relay URL.
    pub relay: String,
    /// Seconds from sending the first request to the last verified byte. Handshakes are excluded.
    pub elapsed_s: f64,
    /// `size_bytes` x 8 / `elapsed_s`, in 10^9 bits per second.
    pub throughput_gbit_s: f64,
    /// `relay` if any connection's selected path is a relay, else `direct`.
    pub path: &'static str,
    /// RTT of the first connection's selected path at the end of the transfer.
    pub rtt_ms: Option<f64>,
    /// User CPU seconds of this process during the transfer. `null` on Windows.
    pub cpu_user_s: Option<f64>,
    /// System CPU seconds of this process during the transfer. `null` on Windows.
    pub cpu_sys_s: Option<f64>,
    /// Negotiated TLS key exchange group.
    pub key_exchange_group: String,
    /// Always `null`: `noq` only reports GSO segments per socket, and iroh does not expose it.
    pub gso_segments: Option<u64>,
    /// Always `null`: iroh counts GRO batches, not the segments in them.
    pub gro_segments: Option<u64>,
    /// QUIC datagrams this endpoint received during the transfer (iroh `recv_datagrams`).
    pub recv_datagrams: u64,
    /// Receive batches that used GRO during the transfer (iroh `recv_gro_datagrams`).
    pub recv_gro_batches: u64,
    /// Locked `iroh` version.
    pub iroh_version: &'static str,
    /// Locked `iroh-blobs` version.
    pub iroh_blobs_version: &'static str,
    /// Root every byte was verified against, in blobs mode.
    pub verified_hash: Option<String>,
}

impl Output {
    /// The result as a single line.
    pub fn line(&self) -> Result<String> {
        serde_json::to_string(self).context("serialize result")
    }
}
