//! What `noq` counted on each connection, so a stalled run can be compared with a good one. Both
//! ends build the same report, each from its own side of the connection.

use std::time::Duration;

use iroh::endpoint::{Connection, PathList};
use noq::congestion::Controller;
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
    /// Congestion window of the selected path, in bytes. `null` while no path can be chosen, and
    /// when the path closed before its controller could be read.
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
    /// Where `paths` came from: `live` at the report, `stream` for the path-change event the list
    /// was read at, `sample` for the once-a-second reading, or `none`.
    pub paths_source: Source,
    /// This end's controller once a second during the transfer. Empty for a transfer shorter than
    /// a second.
    pub controller_series: Vec<Sample>,
}

/// Which reading `ConnReport::paths` is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    /// The list `Connection::paths` returned when the report was built.
    Live,
    /// The list read when `Connection::paths_stream` reported a change.
    Stream,
    /// The list read with a once-a-second controller sample.
    Sample,
    /// No reading ever had a path.
    None,
}

/// What the sampler of one connection collects: the controller series, and the newest path list
/// that had a path in it, whichever of the stream and the sampler read it.
#[derive(Default)]
pub struct Series {
    samples: Vec<Sample>,
    kept: Option<Kept>,
}

/// A path list, when it was read on the sampling clock, and what read it.
struct Kept {
    at: Duration,
    source: Source,
    paths: Vec<PathReport>,
}

impl Series {
    /// Adds a controller sample and keeps the path list. Called once a second.
    pub fn tick(&mut self, conn: &Connection, at: Duration) {
        self.samples.push(sample(conn, at));
        self.keep(Source::Sample, read_paths(conn), at);
    }

    /// Keeps the list a `paths_stream()` item carried. `at` is when the item was received.
    pub fn changed(&mut self, paths: Vec<PathReport>, at: Duration) {
        self.keep(Source::Stream, paths, at);
    }

    /// Replaces the kept list with a newer one. Both sources are read by one task against one
    /// clock, so the later call is the newer reading. An empty list is what the close leaves
    /// behind, and never replaces one with a path in it.
    fn keep(&mut self, source: Source, paths: Vec<PathReport>, at: Duration) {
        if !paths.is_empty() {
            self.kept = Some(Kept { at, source, paths });
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
    let controller = in_use.and_then(|p| conn.congestion_state(p.id()));
    reading(elapsed, controller.as_deref())
}

/// A sample taken from one controller. A path can close between choosing it and asking `noq` for
/// its controller, so the window comes from the controller too: a window with no state would
/// pass for a reading when it is half of one.
fn reading(elapsed: Duration, ctl: Option<&dyn Controller>) -> Sample {
    let read = ctl.map(controller::read);
    Sample {
        elapsed_s: (elapsed.as_secs_f64() * 1000.0).round() / 1000.0,
        cwnd_bytes: ctl.map(Controller::window),
        pacing_rate_bytes_s: read.as_ref().and_then(|c| c.pacing_rate_bytes_s),
        bbr_state: read.as_ref().and_then(|c| c.bbr_state.clone()),
        bbr_max_bw_bytes_s: read.as_ref().and_then(|c| c.bbr_max_bw_bytes_s),
        bbr_app_limited: read.as_ref().and_then(|c| c.bbr_app_limited),
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
    let (paths, paths_at_s, paths_source) = choose(read_paths(conn), now, series.kept);
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
        paths_source,
        controller_series: series.samples,
    }
}

/// The open paths of `conn`, each read once.
fn read_paths(conn: &Connection) -> Vec<PathReport> {
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

/// The live list if it has a path, else the kept one, else none. `Connection::paths` leaves out
/// closed paths (iroh 1.3.0 `connection.rs:1171-1177`), so once the peer has closed the connection
/// the live list can be empty. `at` is when the live list was read.
fn choose(
    live: Vec<PathReport>,
    at: Duration,
    kept: Option<Kept>,
) -> (Vec<PathReport>, Option<f64>, Source) {
    if !live.is_empty() {
        return (live, Some(at.as_secs_f64()), Source::Live);
    }
    match kept {
        Some(kept) => (kept.paths, Some(kept.at.as_secs_f64()), kept.source),
        None => (Vec::new(), None, Source::None),
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
    use std::{sync::Arc, time::Instant};

    use noq::congestion::{Bbr3Config, ControllerFactory, CubicConfig};

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

    fn kept(at_s: u64, source: Source, remote_addr: &str) -> Option<Kept> {
        Some(Kept {
            at: Duration::from_secs(at_s),
            source,
            paths: vec![path(remote_addr)],
        })
    }

    #[test]
    fn a_live_list_wins_and_is_stamped_with_the_report_time() {
        let now = Duration::from_millis(4_250);
        let kept = kept(3, Source::Stream, "kept");
        let (paths, at, source) = choose(vec![path("live")], now, kept);
        assert_eq!(paths[0].remote_addr, "live");
        assert_eq!(at, Some(4.25));
        assert_eq!(source, Source::Live);
    }

    #[test]
    fn the_kept_list_stands_in_for_an_empty_live_one_with_its_own_time_and_source() {
        let kept = kept(3, Source::Sample, "kept");
        let (paths, at, source) = choose(Vec::new(), Duration::from_secs(4), kept);
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0].remote_addr, "kept");
        assert_eq!(at, Some(3.0));
        assert_eq!(source, Source::Sample);
    }

    #[test]
    fn with_neither_there_are_no_paths_and_no_time() {
        let (paths, at, source) = choose(Vec::new(), Duration::from_secs(4), None);
        assert!(paths.is_empty());
        assert_eq!(at, None);
        assert_eq!(source, Source::None);
    }

    #[test]
    fn the_newer_reading_is_kept_and_an_empty_one_is_ignored() {
        let mut series = Series::default();
        series.keep(Source::Sample, vec![path("a")], Duration::from_secs(1));
        series.keep(
            Source::Stream,
            vec![path("b")],
            Duration::from_millis(1_400),
        );
        series.keep(Source::Stream, Vec::new(), Duration::from_secs(2));
        let kept = series.kept.expect("a list was kept");
        assert_eq!(kept.source, Source::Stream);
        assert_eq!(kept.paths[0].remote_addr, "b");
        assert_eq!(kept.at, Duration::from_millis(1_400));
    }

    #[test]
    fn a_sample_with_no_controller_is_null_apart_from_its_time() {
        let sample = reading(Duration::from_millis(3_013), None);
        assert_eq!(sample.elapsed_s, 3.013);
        assert!(sample.cwnd_bytes.is_none());
        assert!(sample.pacing_rate_bytes_s.is_none());
        assert!(sample.bbr_state.is_none());
        assert!(sample.bbr_max_bw_bytes_s.is_none());
        assert!(sample.bbr_app_limited.is_none());
    }

    #[test]
    fn a_sample_takes_the_window_and_the_state_from_one_controller() {
        let bbr = Arc::new(Bbr3Config::default()).build(Instant::now(), 1452);
        let sample = reading(Duration::from_secs(1), Some(bbr.as_ref()));
        assert_eq!(sample.cwnd_bytes, Some(bbr.window()));
        assert_eq!(sample.bbr_state.as_deref(), Some("Startup"));

        let cubic = Arc::new(CubicConfig::default()).build(Instant::now(), 1452);
        let sample = reading(Duration::from_secs(1), Some(cubic.as_ref()));
        assert_eq!(sample.cwnd_bytes, Some(cubic.window()));
        assert!(sample.bbr_state.is_none());
    }
}
