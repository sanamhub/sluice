//! The sending side's view of each connection, printed as it closes.

use std::io::Write;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use iroh::{
    endpoint::Connection,
    protocol::{AcceptError, ProtocolHandler},
};
use iroh_blobs::BlobsProtocol;
use serde::Serialize;
use tokio::{
    sync::mpsc::{self, UnboundedReceiver, UnboundedSender},
    time::{Instant, MissedTickBehavior},
};

use crate::{
    cpu::{self, Cpu},
    raw::RawHandler,
    stats,
};

/// What `serve` answers requests with.
#[derive(Clone, Debug)]
enum Served {
    Raw(RawHandler),
    Blobs(BlobsProtocol),
}

/// The queue the connection lines go through, and the CPU reading they are measured against.
#[derive(Clone, Debug)]
pub struct Lines {
    queue: UnboundedSender<String>,
    cpu_at_ticket: Arc<OnceLock<Cpu>>,
}

/// A queue for the lines and the receiving end `print_lines` drains.
pub fn channel() -> (Lines, UnboundedReceiver<String>) {
    let (queue, receiver) = mpsc::unbounded_channel();
    let lines = Lines {
        queue,
        cpu_at_ticket: Arc::default(),
    };
    (lines, receiver)
}

impl Lines {
    /// Reads this process's CPU time as the zero point for every line printed after it. `serve`
    /// calls it as it prints the ticket, so the startup work, such as generating the data and
    /// hashing the blob, is not charged to the transfer.
    pub fn mark_ticket(&self) {
        if let Some(now) = cpu::now() {
            // Called once, so a second value cannot arrive.
            let _ = self.cpu_at_ticket.set(now);
        }
    }

    /// CPU time the process has used since the ticket. `None` where it cannot be read, or if no
    /// ticket has been marked.
    fn cpu_since_ticket(&self) -> Option<Cpu> {
        cpu::now()
            .zip(self.cpu_at_ticket.get().copied())
            .map(|(now, ticket)| now.since(ticket))
    }
}

/// A protocol handler that also reports every connection it accepted once that connection is
/// closed. The statistics are read after the close because the counters are final then, and
/// `noq` still holds the controller state of the closed connection.
#[derive(Clone, Debug)]
pub struct Reporting {
    served: Served,
    lines: Lines,
}

impl Reporting {
    /// Serves raw streams.
    pub fn raw(handler: RawHandler, lines: Lines) -> Self {
        Self {
            served: Served::Raw(handler),
            lines,
        }
    }

    /// Serves blobs.
    pub fn blobs(protocol: BlobsProtocol, lines: Lines) -> Self {
        Self {
            served: Served::Blobs(protocol),
            lines,
        }
    }
}

impl ProtocolHandler for Reporting {
    async fn accept(&self, conn: Connection) -> Result<(), AcceptError> {
        tokio::spawn(report_on_close(conn.clone(), self.lines.clone()));
        match &self.served {
            Served::Raw(handler) => handler.accept(conn).await,
            Served::Blobs(protocol) => protocol.accept(conn).await,
        }
    }
}

/// The `connection_closed` line.
#[derive(Serialize)]
struct Closed {
    event: &'static str,
    /// Why the connection ended, as `noq` words it.
    close_reason: String,
    /// User CPU seconds `serve` has used since it printed the ticket, up to this close. The
    /// process is shared, so with several connections the line of the last one to close has the
    /// whole transfer. `null` on Windows.
    cpu_user_s: Option<f64>,
    /// System CPU seconds, counted the same way.
    cpu_sys_s: Option<f64>,
    #[serde(flatten)]
    connection: stats::ConnReport,
}

/// Writes each line to stdout and flushes it, so a harness that reads the pipe sees a line as soon
/// as its connection closes. One task writes them all, so lines never interleave.
pub async fn print_lines(mut lines: UnboundedReceiver<String>) {
    let mut out = std::io::stdout();
    while let Some(line) = lines.recv().await {
        if let Err(e) = writeln!(out, "{line}").and_then(|()| out.flush()) {
            tracing::warn!("could not write a connection line: {e}");
        }
    }
}

/// Reads the connection once a second until it closes, then prints the closing counters with the
/// series. The clock starts at accept, so it runs a handshake ahead of the receiver's `progress`.
async fn report_on_close(conn: Connection, lines: Lines) {
    let second = Duration::from_secs(1);
    let origin = Instant::now();
    let mut clock = tokio::time::interval_at(origin + second, second);
    // Same rule as the receiver: a late tick must not be followed by a burst.
    clock.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut series = Vec::new();
    let closed = conn.closed();
    tokio::pin!(closed);
    let reason = loop {
        tokio::select! {
            reason = &mut closed => break reason,
            _ = clock.tick() => series.push(stats::sample(&conn, origin.elapsed())),
        }
    };
    let cpu = lines.cpu_since_ticket();
    let closed = Closed {
        event: "connection_closed",
        close_reason: reason.to_string(),
        cpu_user_s: cpu.map(|c| c.user_s),
        cpu_sys_s: cpu.map(|c| c.sys_s),
        connection: stats::report(&conn, None, series),
    };
    match serde_json::to_string(&closed) {
        // A closed channel means `serve` is shutting down without waiting for the line.
        Ok(line) => drop(lines.queue.send(line)),
        Err(e) => tracing::warn!("could not serialize a connection report: {e}"),
    }
}
