//! The stops a send asks the provider for, and the grace period that follows a delivered root.

use std::{sync::Arc, time::Duration};

use crate::engine::live::LiveLog;

/// When `send` stops serving.
#[derive(Clone, Debug)]
pub enum Stop {
    /// After every content root has been served and the receiver has closed its connection.
    OneDownload(Vec<[u8; 32]>),
    /// A live stream: serve until its close is signed, then until every segment is delivered.
    Live(Arc<LiveLog>),
    /// On Ctrl+C or at the ticket expiry, and with a limit, once this many distinct receivers have
    /// each taken the whole transfer. `roots` is what a completion has to have asked for to count.
    Keep {
        /// How many receivers that took the whole transfer end the send, or none for a wait that
        /// only Ctrl+C or the expiry ends.
        limit: Option<u32>,
        /// The content roots the transfer offers, which a completion code is checked against.
        roots: Vec<[u8; 32]>,
    },
}

/// Why serving ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServeEnd {
    /// The one download finished.
    Downloaded,
    /// The ticket expired and the provider stopped serving it.
    Expired,
    /// Ctrl+C.
    Interrupted,
}

/// The grace period after the last content root is delivered while a receiver keeps its connection
/// open. A test uses a short one so the rule can be watched without waiting half a minute.
pub(super) const GRACE: Duration = Duration::from_secs(if cfg!(test) { 2 } else { 30 });

/// Waits for the grace period, or for ever when none has started. A plain `Option` cannot be
/// selected on, and a fresh sleep would restart the period on every loop pass.
pub(super) async fn wait_grace(grace: &mut Option<std::pin::Pin<Box<tokio::time::Sleep>>>) {
    match grace {
        Some(sleep) => sleep.as_mut().await,
        None => std::future::pending::<()>().await,
    }
}
