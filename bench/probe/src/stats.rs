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
    /// The paths open when the report was built, or the last list this end read while any was open,
    /// if the peer had closed them by then. See `paths_at_s`.
    pub paths: Vec<PathReport>,
    /// Seconds, on the clock of `controller_series`, at which `paths` was read. It is the report
    /// time for a live list and earlier for a kept one, whose counters are that much older. `null`
    /// when `paths` is empty because no list with a path was ever read.
    pub paths_at_s: Option<f64>,
    /// This end's controller once a second during the transfer. Empty for a transfer shorter than
    /// a second.
    pub controller_series: Vec<Sample>,
}

/// What the sampler of one connection collects: the controller series, and the latest path list
/// that had a path in it.
#[derive(Default)]
pub struct Series {
    samples: Vec<Sample>,
    kept: Option<Kept>,
}

/// A path list and the time it was read, as a duration on the sampling clock.
struct Kept {
    at: Duration,
    paths: Vec<PathReport>,
}

impl Series {
    /// Adds a controller sample and keeps the path list. Called once a second.
    pub fn tick(&mut self, conn: &Connection, at: Duration) {
        self.samples.push(sample(conn, at));
        self.keep_paths(conn, at);
    }

    /// Keeps the path list without adding a sample. Called as the transfer starts, because a
    /// transfer under a second is closed by the peer before any tick, and its report would have
    /// nothing to fall back on.
    pub fn keep_paths(&mut self, conn: &Connection, at: Duration) {
        let paths = read_paths(conn);
        if !paths.is_empty() {
            self.kept = Some(Kept { at, paths });
        }
    }
}

/// Reads `conn`'s controller on its selected path, or on its only open path. With several open and
/// none selected there is no honest choice, so the reading is `null`.
fn sample(conn: &Connection, elapsed: Duration) -> Sample {
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

/// Reads the counters of `conn`. `series` is what the caller sampled while the transfer ran, and
/// `now` is the time of this call on the same clock.
pub fn report(
    conn: &Connection,
    received_bytes: Option<u64>,
    series: Series,
    now: Duration,
) -> ConnReport {
    let total = conn.stats();
    let (paths, paths_at_s) = choose(read_paths(conn), now, series.kept);
    ConnReport {
        key: key(conn),
        remote_id: conn.remote_id().to_string(),
        received_bytes,
        sent_datagrams: total.udp_tx.datagrams,
        received_datagrams: total.udp_rx.datagrams,
        lost_packets: total.lost_packets,
        lost_bytes: total.lost_bytes,
        paths,
        paths_at_s,
        controller_series: series.samples,
    }
}

/// The open paths of `conn`, each read once.
fn read_paths(conn: &Connection) -> Vec<PathReport> {
    conn.paths()
        .iter()
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

/// The live list if it has a path, else the kept one, else none. `Connection::paths` leaves out
/// closed paths (iroh 1.3.0 `connection.rs:1171-1177`), so once the peer has closed the connection
/// the live list can be empty. `at` is when the live list was read.
fn choose(
    live: Vec<PathReport>,
    at: Duration,
    kept: Option<Kept>,
) -> (Vec<PathReport>, Option<f64>) {
    if !live.is_empty() {
        return (live, Some(at.as_secs_f64()));
    }
    match kept {
        Some(kept) => (kept.paths, Some(kept.at.as_secs_f64())),
        None => (Vec::new(), None),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn path(remote_addr: &str) -> PathReport {
        PathReport {
            selected: false,
            relay: false,
            remote_addr: remote_addr.to_string(),
            rtt_ms: 0.0,
            cwnd_bytes: 0,
            congestion_events: 0,
            spurious_congestion_events: 0,
            lost_packets: 0,
            lost_bytes: 0,
            sent_datagrams: 0,
            received_datagrams: 0,
            mtu: 0,
            controller: None,
        }
    }

    fn kept(at_s: u64, remote_addr: &str) -> Option<Kept> {
        Some(Kept {
            at: Duration::from_secs(at_s),
            paths: vec![path(remote_addr)],
        })
    }

    #[test]
    fn a_live_list_wins_and_is_stamped_with_the_report_time() {
        let now = Duration::from_millis(4_250);
        let (paths, at) = choose(vec![path("live")], now, kept(3, "kept"));
        assert_eq!(paths[0].remote_addr, "live");
        assert_eq!(at, Some(4.25));
    }

    #[test]
    fn the_kept_list_stands_in_for_an_empty_live_one_with_its_own_time() {
        let (paths, at) = choose(Vec::new(), Duration::from_secs(4), kept(3, "kept"));
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0].remote_addr, "kept");
        assert_eq!(at, Some(3.0));
    }

    #[test]
    fn with_neither_there_are_no_paths_and_no_time() {
        let (paths, at) = choose(Vec::new(), Duration::from_secs(4), None);
        assert!(paths.is_empty());
        assert_eq!(at, None);
    }
}
