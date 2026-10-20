//! What `noq` counted on each connection, so a stalled run can be compared with a good one. Both
//! ends build the same report, each from its own side of the connection.

use std::time::Duration;

use iroh::endpoint::{Connection, PathList};
use serde::Serialize;

use crate::{
    controller::{self, ControllerReport},
    sampling::{Sample, Series, Source},
};

/// Label of the exported TLS keying material that both ends use as the connection's join key.
const KEY_LABEL: &[u8] = b"sluice-probe/connection-key";

/// One open path of a connection. Congestion state is per path, and a connection can hold a direct
/// and a relay path at once.
#[derive(Serialize)]
pub struct PathReport {
    /// iroh's choice for application data. It marks a path late, so a short run can show none.
    pub selected: bool,
    /// Whether the path goes through a relay.
    pub relay: bool,
    /// The address the other end is reached at on this path: `ip:port`, or the relay URL.
    pub remote_addr: String,
    /// Round-trip time estimate.
    pub rtt_ms: f64,
    /// Congestion window of this end's controller, in bytes. In a download only the sender's
    /// window limits throughput, so read it on the `serve` line.
    pub cwnd_bytes: u64,
    /// Congestion events on this path.
    pub congestion_events: u64,
    /// Congestion events that later turned out not to be losses.
    pub spurious_congestion_events: u64,
    /// Packets this end sent that were declared lost.
    pub lost_packets: u64,
    /// Bytes this end sent that were declared lost.
    pub lost_bytes: u64,
    /// UDP datagrams sent on this path.
    pub sent_datagrams: u64,
    /// UDP datagrams received on this path.
    pub received_datagrams: u64,
    /// Largest UDP payload the path supports.
    pub mtu: u16,
    /// This end's congestion controller. `null` if `noq` no longer holds one for the path.
    pub controller: Option<ControllerReport>,
}

/// One connection: payload bytes counted by the probe, and what `noq` counted around them.
#[derive(Serialize)]
pub struct ConnReport {
    /// Identifies the connection on both ends: the same on the `fetch` entry and the `serve`
    /// line. 8 bytes of TLS keying material in hex, so it is unique per connection and nothing
    /// extra crosses the wire. `null` if the keying material could not be exported.
    pub key: Option<String>,
    /// The other end's endpoint id. Several connections of one `fetch` share it.
    pub remote_id: String,
    /// Payload bytes received on this connection so far. `null` on `serve`, which does not count
    /// what it sends.
    pub received_bytes: Option<u64>,
    /// UDP datagrams sent over all paths the connection ever had. `noq` does not count QUIC
    /// packets, so this is the closest it exposes to a sent packet count.
    pub sent_datagrams: u64,
    /// UDP datagrams received over all paths the connection ever had.
    pub received_datagrams: u64,
    /// Packets this end sent that were declared lost, over all paths.
    pub lost_packets: u64,
    /// Bytes this end sent that were declared lost, over all paths.
    pub lost_bytes: u64,
    /// The paths open when the report was built, or the last list this end read while any was open,
    /// if the peer had closed them by then. See `paths_at_s`.
    pub paths: Vec<PathReport>,
    /// Seconds, on the clock of `controller_series`, at which `paths` was read. It is the report
    /// time for a live list and earlier for a kept one, whose counters are that much older. `null`
    /// when `paths` is empty because no list with a path was ever read.
    pub paths_at_s: Option<f64>,
    /// Where `paths` came from: `live` at the report, `stream` for the path-change event the list
    /// was read at, `sample` for the once-a-second reading, or `none`.
    pub paths_source: Source,
    /// This end's controller once a second during the transfer. Empty for a transfer shorter than
    /// a second.
    pub controller_series: Vec<Sample>,
}

/// Reads the counters of `conn`. `series` is what the caller sampled while the transfer ran, and
/// `now` is the time of this call on the same clock.
pub fn report(
    conn: &Connection,
    received_bytes: Option<u64>,
    series: Series,
    now: Duration,
) -> ConnReport {
    let total = conn.stats();
    let closing = series.close(read_paths(conn), now);
    ConnReport {
        key: key(conn),
        remote_id: conn.remote_id().to_string(),
        received_bytes,
        sent_datagrams: total.udp_tx.datagrams,
        received_datagrams: total.udp_rx.datagrams,
        lost_packets: total.lost_packets,
        lost_bytes: total.lost_bytes,
        paths: closing.paths,
        paths_at_s: closing.paths_at_s,
        paths_source: closing.paths_source,
        controller_series: closing.samples,
    }
}

/// The open paths of `conn`, each read once.
pub fn read_paths(conn: &Connection) -> Vec<PathReport> {
    read_list(conn, &conn.paths())
}

/// The paths in `list`, which `conn` produced.
pub fn read_list(conn: &Connection, list: &PathList<'_>) -> Vec<PathReport> {
    list.iter()
        .map(|path| {
            let s = path.stats();
            PathReport {
                selected: path.is_selected(),
                relay: path.is_relay(),
                remote_addr: path.remote_addr().to_string(),
                rtt_ms: s.rtt.as_secs_f64() * 1000.0,
                cwnd_bytes: s.cwnd,
                congestion_events: s.congestion_events,
                spurious_congestion_events: s.spurious_congestion_events,
                lost_packets: s.lost_packets,
                lost_bytes: s.lost_bytes,
                sent_datagrams: s.udp_tx.datagrams,
                received_datagrams: s.udp_rx.datagrams,
                mtu: s.current_mtu,
                controller: conn
                    .congestion_state(path.id())
                    .map(|c| controller::read(c.as_ref())),
            }
        })
        .collect()
}

/// Both ends derive the same bytes from the TLS session, so no connection counter has to be
/// negotiated, and connections that share an endpoint id still get different keys.
fn key(conn: &Connection) -> Option<String> {
    let mut bytes = [0u8; 8];
    conn.export_keying_material(&mut bytes, KEY_LABEL, b"")
        .ok()?;
    Some(bytes.iter().map(|b| format!("{b:02x}")).collect())
}
