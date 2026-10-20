//! The once-a-second readings of a connection: the controller series, and the path list kept for the
//! report when the peer closes the connection before this end reads it.

use std::{cmp::Reverse, time::Duration};

use iroh::endpoint::Connection;
use noq::{PathId, congestion::Controller};
use serde::Serialize;

use crate::{
    controller,
    stats::{PathReport, read_paths},
};

/// One reading of a connection's controller, taken once a second while the transfer runs. The
/// closing snapshot cannot show when a collapse started, and a series of these can.
#[derive(Serialize)]
pub struct Sample {
    /// Seconds on the sampling clock, to the millisecond. In a `fetch` entry the clock is the one
    /// `progress` uses, so entry `i` pairs with `progress[i]`. In a `serve` line it starts when the
    /// connection is accepted, a little before the transfer.
    pub elapsed_s: f64,
    /// How many paths were open when the sample was read.
    pub open_paths: usize,
    /// Which path the sample read: `selected` for the one iroh marked, `only` for a single open
    /// path, `busiest` when several are open and none is selected, or `null` when none is open.
    /// The choice is recorded even if the path closed before its controller was read, which shows
    /// as `null` fields below.
    pub path: Option<Chosen>,
    /// Congestion window of that path, in bytes. `null` while no path can be chosen, and
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

/// How a path was chosen.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Chosen {
    /// iroh marked it selected.
    Selected,
    /// It is the only open path.
    Only,
    /// Several paths are open and none is selected, so it is the one with the most datagrams.
    Busiest,
}

/// What choosing a path needs to know about one open path.
struct Candidate {
    id: PathId,
    selected: bool,
    /// Sent plus received over the path's life.
    datagrams: u64,
}

/// The path to read, as an index into `open`, and why it was chosen. iroh can hold two paths for a
/// whole transfer without marking one, and a reading that is `null` then says nothing while data
/// moves, so the path that has carried the most datagrams stands in. A tie goes to the lower path
/// id, so the choice does not depend on the order of the list.
fn pick(open: &[Candidate]) -> Option<(usize, Chosen)> {
    if let Some(i) = open.iter().position(|c| c.selected) {
        return Some((i, Chosen::Selected));
    }
    if open.len() == 1 {
        return Some((0, Chosen::Only));
    }
    open.iter()
        .enumerate()
        .max_by_key(|(_, c)| (c.datagrams, Reverse(c.id)))
        .map(|(i, _)| (i, Chosen::Busiest))
}

/// The path `pick` chooses among `paths`. `fetch` summarises with the same path a sample reads.
pub fn in_use(paths: &[PathReport]) -> Option<&PathReport> {
    let open: Vec<Candidate> = paths
        .iter()
        .map(|p| Candidate {
            id: p.id,
            selected: p.selected,
            datagrams: p.sent_datagrams + p.received_datagrams,
        })
        .collect();
    pick(&open).map(|(i, _)| &paths[i])
}

/// Reads `conn`'s controller on the path `pick` chooses.
fn sample(conn: &Connection, elapsed: Duration) -> Sample {
    let list = conn.paths();
    let open: Vec<Candidate> = list
        .iter()
        .map(|p| {
            let stats = p.stats();
            Candidate {
                id: p.id(),
                selected: p.is_selected(),
                datagrams: stats.udp_tx.datagrams + stats.udp_rx.datagrams,
            }
        })
        .collect();
    let picked = pick(&open);
    let controller = picked
        .and_then(|(i, _)| list.iter().nth(i))
        .and_then(|p| conn.congestion_state(p.id()));
    reading(
        elapsed,
        open.len(),
        picked.map(|(_, chosen)| chosen),
        controller.as_deref(),
    )
}

/// A sample taken from one controller. A path can close between choosing it and asking `noq` for
/// its controller, so the window comes from the controller too: a window with no state would
/// pass for a reading when it is half of one.
fn reading(
    elapsed: Duration,
    open_paths: usize,
    path: Option<Chosen>,
    ctl: Option<&dyn Controller>,
) -> Sample {
    let read = ctl.map(controller::read);
    Sample {
        elapsed_s: (elapsed.as_secs_f64() * 1000.0).round() / 1000.0,
        open_paths,
        path,
        cwnd_bytes: ctl.map(Controller::window),
        pacing_rate_bytes_s: read.as_ref().and_then(|c| c.pacing_rate_bytes_s),
        bbr_state: read.as_ref().and_then(|c| c.bbr_state.clone()),
        bbr_max_bw_bytes_s: read.as_ref().and_then(|c| c.bbr_max_bw_bytes_s),
        bbr_app_limited: read.as_ref().and_then(|c| c.bbr_app_limited),
    }
}

/// What a connection's series leaves for the report.
pub struct Closing {
    pub samples: Vec<Sample>,
    pub paths: Vec<PathReport>,
    pub paths_at_s: Option<f64>,
    pub paths_source: Source,
}

impl Series {
    /// Picks the path list for the report, the live one at `now` or the kept one, and hands over the
    /// samples.
    pub fn close(self, live: Vec<PathReport>, now: Duration) -> Closing {
        let (paths, paths_at_s, paths_source) = choose(live, now, self.kept);
        Closing {
            samples: self.samples,
            paths,
            paths_at_s,
            paths_source,
        }
    }
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

#[cfg(test)]
mod tests;
