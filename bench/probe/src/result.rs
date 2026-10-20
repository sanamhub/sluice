//! The one JSON line `fetch` prints.

use anyhow::{Context, Result};
use serde::Serialize;

use crate::{
    cli::{Congestion, Mode, Sink, StoreKind},
    net::Windows,
    rendezvous::Discovery,
    stats::ConnReport,
};

/// How the transfer ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    /// Every byte arrived and verified.
    Ok,
    /// `--timeout` expired first. The counters are what had been measured by then.
    Timeout,
    /// The dial failed or did not finish within `--timeout`, so no connection was ever up. Without a
    /// relay that is the answer to whether the two ends could reach each other.
    #[serde(rename = "no_path")]
    NoPath,
}

/// A transfer, finished or cut off by `--timeout`. Fields iroh does not expose are `null`, never
/// estimated.
#[derive(Serialize)]
pub struct Output {
    /// `ok`, `timeout` or `no_path`. The last two are still results, and `fetch` exits non-zero after
    /// printing them.
    pub outcome: Outcome,
    /// `raw` or `blobs`.
    pub mode: Mode,
    /// Size the ticket promised.
    pub size_bytes: u64,
    /// Payload bytes counted at the receiver. Equals `size_bytes` when the outcome is `ok`.
    pub received_bytes: u64,
    /// Parallel connections used.
    pub connections: u16,
    /// Endpoints `serve` bound, which is the number of addresses in the ticket.
    pub serve_endpoints: usize,
    /// Endpoints `fetch` bound: 1 with `--endpoints shared`, `connections` with `separate`.
    pub fetch_endpoints: usize,
    /// `mem` or `fs` in blobs mode, `null` in raw mode, which has no store.
    pub serve_store: Option<StoreKind>,
    /// Congestion controller on this end.
    pub congestion: Congestion,
    /// Windows on this end.
    pub window: Windows,
    /// `discard` or `store` in blobs mode, `null` in raw mode.
    pub sink: Option<Sink>,
    /// `disabled` or the relay URL.
    pub relay: String,
    /// The rendezvous server's URL, `null` without one.
    pub rendezvous: Option<String>,
    /// What the rendezvous server told `fetch`, and how long it took. `null` without one.
    pub discovery: Option<Discovery>,
    /// Seconds from starting the dial to the connections being up, after address discovery. `null`
    /// for `no_path`. With no relay, a connection that is up is a direct one.
    pub connect_s: Option<f64>,
    /// Why a `no_path` dial failed, as iroh words it. `null` otherwise.
    pub error: Option<String>,
    /// Seconds from sending the first request to the last verified byte, or to the timeout.
    /// Handshakes are excluded.
    pub elapsed_s: f64,
    /// `received_bytes` x 8 / `elapsed_s`, in 10^9 bits per second.
    pub throughput_gbit_s: f64,
    /// Total payload bytes received at the end of each whole second of the transfer. A stall shows
    /// as the point where the numbers stop growing.
    pub progress: Vec<u64>,
    /// What `noq` counted on each connection, in the order the connections were opened.
    pub connection_stats: Vec<ConnReport>,
    /// `relay` if any connection's selected path is a relay. `direct` if none is, or if no open path
    /// is a relay. `null` when iroh had not marked a path selected and a relay path was open.
    pub path: Option<&'static str>,
    /// RTT of the first connection's selected path at the end of the transfer. `null` if iroh had
    /// not marked a path selected and the connection had several open.
    pub rtt_ms: Option<f64>,
    /// User CPU seconds of this process during the transfer. `null` on Windows.
    pub cpu_user_s: Option<f64>,
    /// System CPU seconds of this process during the transfer. `null` on Windows.
    pub cpu_sys_s: Option<f64>,
    /// Negotiated TLS key exchange group. `null` for `no_path`.
    pub key_exchange_group: Option<String>,
    /// Always `null`: `noq` only reports GSO segments per socket, and iroh does not expose it.
    pub gso_segments: Option<u64>,
    /// Always `null`: iroh counts GRO batches, not the segments in them.
    pub gro_segments: Option<u64>,
    /// QUIC datagrams the endpoints received during the transfer (iroh `recv_datagrams`, summed).
    pub recv_datagrams: u64,
    /// Receive batches that used GRO during the transfer (iroh `recv_gro_datagrams`).
    pub recv_gro_batches: u64,
    /// Locked `iroh` version.
    pub iroh_version: &'static str,
    /// Locked `iroh-blobs` version.
    pub iroh_blobs_version: &'static str,
    /// Root every byte was verified against, in blobs mode. After a timeout, the root they would
    /// have been checked against.
    pub verified_hash: Option<String>,
}

impl Output {
    /// The result as a single line.
    pub fn line(&self) -> Result<String> {
        serde_json::to_string(self).context("serialize result")
    }
}
