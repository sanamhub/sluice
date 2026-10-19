//! What `noq` counted on each connection, so a stalled run can be compared with a good one. Both
//! ends build the same report, each from its own side of the connection.

use std::time::Duration;

use iroh::endpoint::Connection;
use serde::Serialize;

use crate::controller::{self, ControllerReport};

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

/// One reading of a connection's controller, taken once a second while the transfer runs. The
/// closing snapshot cannot show when a collapse started, and a series of these can.
#[derive(Serialize)]
pub struct Sample {
    /// Seconds on the sampling clock, to the millisecond. In a `fetch` entry the clock is the one
    /// `progress` uses, so entry `i` pairs with `progress[i]`. In a `serve` line it starts when the
    /// connection is accepted, a little before the transfer.
    pub elapsed_s: f64,
    /// Congestion window of the selected path, in bytes. `null` while no path can be chosen.
    pub cwnd_bytes: Option<u64>,
    /// Pacing rate in bytes per second. `null` for controllers that do not pace.
    pub pacing_rate_bytes_s: Option<u64>,
    /// BBR's state. `null` for other controllers.
    pub bbr_state: Option<String>,
    /// BBR's bottleneck bandwidth estimate in bytes per second. `null` for other controllers.
    pub bbr_max_bw_bytes_s: Option<f64>,
    /// Whether BBR counts the sender as application-limited. `null` for other controllers.
    pub bbr_app_limited: Option<bool>,
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
    /// The paths open now.
    pub paths: Vec<PathReport>,
    /// This end's controller once a second during the transfer. Empty for a transfer shorter than
    /// a second.
    pub controller_series: Vec<Sample>,
}

/// Reads `conn`'s controller on its selected path, or on its only open path. With several open and
/// none selected there is no honest choice, so the reading is `null`.
pub fn sample(conn: &Connection, elapsed: Duration) -> Sample {
    let paths = conn.paths();
    let in_use = paths
        .iter()
        .find(|p| p.is_selected())
        .or_else(|| paths.iter().next().filter(|_| paths.len() == 1));
    let controller = in_use
        .as_ref()
        .and_then(|p| conn.congestion_state(p.id()))
        .map(|c| controller::read(c.as_ref()));
    let (pacing, state, max_bw, app_limited) = match controller {
        Some(c) => (
            c.pacing_rate_bytes_s,
            c.bbr_state,
            c.bbr_max_bw_bytes_s,
            c.bbr_app_limited,
        ),
        None => (None, None, None, None),
    };
    Sample {
        elapsed_s: (elapsed.as_secs_f64() * 1000.0).round() / 1000.0,
        cwnd_bytes: in_use.map(|p| p.stats().cwnd),
        pacing_rate_bytes_s: pacing,
        bbr_state: state,
        bbr_max_bw_bytes_s: max_bw,
        bbr_app_limited: app_limited,
    }
}

/// Reads the counters of `conn`. `series` is what the caller sampled while the transfer ran.
pub fn report(conn: &Connection, received_bytes: Option<u64>, series: Vec<Sample>) -> ConnReport {
    let total = conn.stats();
    let paths = conn
        .paths()
        .iter()
        .map(|path| {
            let s = path.stats();
            PathReport {
                selected: path.is_selected(),
                relay: path.is_relay(),
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
        .collect();
    ConnReport {
        key: key(conn),
        remote_id: conn.remote_id().to_string(),
        received_bytes,
        sent_datagrams: total.udp_tx.datagrams,
        received_datagrams: total.udp_rx.datagrams,
        lost_packets: total.lost_packets,
        lost_bytes: total.lost_bytes,
        paths,
        controller_series: series,
    }
}

/// Both ends derive the same bytes from the TLS session, so no connection counter has to be
/// negotiated, and connections that share an endpoint id still get different keys.
fn key(conn: &Connection) -> Option<String> {
    let mut bytes = [0u8; 8];
    conn.export_keying_material(&mut bytes, KEY_LABEL, b"")
        .ok()?;
    Some(bytes.iter().map(|b| format!("{b:02x}")).collect())
}
