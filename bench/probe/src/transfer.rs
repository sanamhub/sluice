//! Runs the per-connection transfers and samples their progress once a second.

use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, Result};
use iroh::endpoint::Connection;
use iroh_blobs::{Hash, api::Store};
use n0_future::{MergeUnbounded, StreamExt};
use tokio::{task::JoinSet, time::MissedTickBehavior};

use crate::{blobs, raw, sampling::Series, stats};

/// What each connection is asked for.
pub enum Plan {
    /// Byte counts for the raw streams.
    Raw(Vec<u64>),
    /// Chunk ranges of one blob, fetched into `store` or verified and dropped.
    Blobs {
        hash: Hash,
        parts: Vec<(u64, u64)>,
        store: Option<Store>,
    },
}

/// Payload bytes received so far on each connection. The transfer tasks write them and the
/// sampler reads them, so a timed-out run still knows how far each connection got.
pub struct Counters(Vec<Arc<AtomicU64>>);

impl Counters {
    /// One counter per connection, all zero.
    pub fn new(connections: usize) -> Self {
        Self((0..connections).map(|_| Arc::default()).collect())
    }

    /// The counter connection `i` writes.
    pub fn handle(&self, i: usize) -> Arc<AtomicU64> {
        Arc::clone(&self.0[i])
    }

    /// Bytes received on each connection.
    pub fn per_connection(&self) -> Vec<u64> {
        self.0.iter().map(|c| c.load(Ordering::Relaxed)).collect()
    }

    /// Bytes received on all connections.
    pub fn total(&self) -> u64 {
        self.per_connection().iter().sum()
    }
}

/// Runs the transfers to the end and returns the payload bytes they report. Pushes the running
/// total onto `progress` once per second, and a controller reading of each connection onto its
/// entry in `series` at the same tick. The interval is a clock: the counters are only read when
/// it fires, and the tasks never wait for it. With `live`, each tick also prints a progress line.
pub async fn run(
    conns: &[Connection],
    plan: Plan,
    counters: &Counters,
    live: bool,
    progress: &mut Vec<u64>,
    series: &mut [Series],
) -> Result<u64> {
    let mut tasks: JoinSet<Result<u64>> = JoinSet::new();
    match plan {
        Plan::Raw(lens) => {
            for (i, (conn, len)) in conns.iter().cloned().zip(lens).enumerate() {
                let counter = counters.handle(i);
                tasks.spawn(async move { raw::receive(&conn, len, &counter).await });
            }
        }
        Plan::Blobs { hash, parts, store } => {
            for (i, (conn, part)) in conns.iter().cloned().zip(parts).enumerate() {
                let counter = counters.handle(i);
                match &store {
                    None => tasks.spawn(blobs::fetch_discard(conn, hash, part, counter)),
                    Some(store) => {
                        tasks.spawn(blobs::fetch_store(store.clone(), conn, hash, part, counter))
                    }
                };
            }
        }
    }

    let second = Duration::from_secs(1);
    let origin = tokio::time::Instant::now();
    let mut clock = tokio::time::interval_at(origin + second, second);
    // A late tick must not be followed by a burst, or the samples stop being one second apart.
    clock.set_missed_tick_behavior(MissedTickBehavior::Delay);
    // Each stream borrows its connection from `conns`, so nothing here outlives the transfer or
    // keeps a connection open. A stream ends when its connection closes.
    let mut changes = MergeUnbounded::from_iter(conns.iter().enumerate().map(|(i, conn)| {
        conn.paths_stream()
            .map(move |list| (i, stats::read_list(conn, &list)))
    }));
    let mut total = 0;
    loop {
        tokio::select! {
            joined = tasks.join_next() => match joined {
                Some(joined) => total += joined.context("transfer task panicked")??,
                None => return Ok(total),
            },
            Some((i, paths)) = changes.next() => series[i].changed(paths, origin.elapsed()),
            _ = clock.tick() => {
                let bytes = counters.total();
                progress.push(bytes);
                if live {
                    // println! flushes at the newline, so a reader on a pipe sees it this second.
                    println!("{}", progress_line(progress.len() as u64, bytes));
                }
                let at = origin.elapsed();
                for (conn, series) in conns.iter().zip(series.iter_mut()) {
                    series.tick(conn, at);
                }
            }
        }
    }
}

/// The line `--progress-lines` prints for second `t`.
fn progress_line(t: u64, bytes: u64) -> String {
    format!(r#"{{"event":"progress","t":{t},"bytes":{bytes}}}"#)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_progress_line_is_json_with_the_second_and_the_bytes() {
        let line = progress_line(3, 1_048_576);
        assert_eq!(line, r#"{"event":"progress","t":3,"bytes":1048576}"#);
        let parsed: serde_json::Value = serde_json::from_str(&line).expect("JSON");
        assert_eq!(parsed["event"], "progress");
        assert_eq!(parsed["t"], 3);
        assert_eq!(parsed["bytes"], 1_048_576);
    }
}
