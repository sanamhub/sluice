//! What `noq` counted on each connection, so a stalled run can be compared with a good one. Both
//! ends build the same report, each from its own side of the connection.

use std::time::Duration;

use iroh::endpoint::{Connection, PathList};
use noq::PathId;
use serde::Serialize;

use crate::{
    controller::{self, ControllerReport},
    sampling::{Sample, Series, Source},
};

/// Label of the exported TLS keying material that both ends use as the connection's join key.
const KEY_LABEL: &[u8] = b"sluice-probe/connection-key";

/// One open path of a connection. Congestion state is per path, and a connection can hold a direct
/// and a relay path at once.
#[derive(Default, Serialize)]
pub struct PathReport {
    /// `noq`'s id for the path. It only breaks ties between equally busy paths, so it is not
    /// printed.
    #[serde(skip)]
    pub id: PathId,
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
    /// The ids, 0 to 63, that `noq` holds a controller for and `paths` does not include, whatever
    /// `paths` holds. It has two shapes. `paths` is empty because iroh never listed the path the
    /// data moved on. Or `paths` lists only paths `noq` no longer holds, and the path in use is
    /// missing from it. Either way this is the evidence, with no address, statistics or RTT to go
    /// with it, since none is known. `[]` when iroh listed every path `noq` holds. A path iroh has
    /// just closed can show here for a moment too, while `noq` still holds its controller. A lower
    /// bound: see `UNLISTED_ID_LIMIT`.
    pub unlisted_path_ids: Vec<u32>,
    /// This end's controller once a second during the transfer. Empty for a transfer shorter than
    /// a second.
    pub controller_series: Vec<Sample>,
}

/// How many path ids `unlisted_path_ids` asks `noq` about. A path id is never reused, and only the
/// client opens paths (`noq-proto` 1.3.0 `connection/mod.rs:565`), numbering the next one after the
/// highest id used or abandoned (`:569-572`). It may not pass `min(local, remote)` of the peers'
/// maximum path id, which starts at `MAX_MULTIPATH_PATHS - 1` = 7 (iroh `socket.rs:137`, `noq-proto`
/// `config/transport.rs:439`) and grows by one for each `PATH_ABANDON` received (`:5372`). So the
/// highest id a connection can reach is 7 plus the paths its peer abandoned, which has no bound,
/// and `noq` has no public accessor for the current maximum. A connection abandons a path a few
/// times at most in the first seconds (id 4 in the runs that motivated this), so 64 is eight
/// times the starting window. An id beyond it is not reported, so the field can miss a path.
const UNLISTED_ID_LIMIT: u32 = 64;

/// The ids below `UNLISTED_ID_LIMIT` that `noq` holds a controller for and `listed` does not
/// include, compared by `PathReport::id`. `holds_controller` answers for one id, so the choice can
/// be tested without a connection. It is asked only about ids iroh did not list.
fn unlisted_path_ids(listed: &[PathReport], holds_controller: impl Fn(PathId) -> bool) -> Vec<u32> {
    (0..UNLISTED_ID_LIMIT)
        .map(|id| (id, PathId::from(id)))
        .filter(|&(_, id)| !listed.iter().any(|p| p.id == id))
        .filter(|&(_, id)| holds_controller(id))
        .map(|(n, _)| n)
        .collect()
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
    let unlisted_path_ids =
        unlisted_path_ids(&closing.paths, |id| conn.congestion_state(id).is_some());
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
        unlisted_path_ids,
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
                id: path.id(),
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

#[cfg(test)]
mod tests;
