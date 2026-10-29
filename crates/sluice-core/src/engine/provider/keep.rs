//! A send that keeps serving: until Ctrl+C or the expiry, and with a limit until every place it
//! offers is taken and the connections of the endpoints holding them have closed.

use std::{
    collections::{BTreeMap, BTreeSet},
    time::Duration,
};

use iroh_blobs::Hash;

use super::{GRACE, Provider, ServeEnd, lock_places, reports_stopped, wait_grace};
use crate::{
    Error,
    engine::{endpoint, now_ms},
};

/// The timer that bounds the wait for a holder that is still connected.
type Grace = std::pin::Pin<Box<tokio::time::Sleep>>;

impl Provider {
    /// Serves until Ctrl+C or the expiry, and with a limit until every place is taken and the
    /// connections of the endpoints holding them have closed. A holder that keeps a connection open
    /// doing nothing is waited for no longer than [`GRACE`] after its last request or byte; activity
    /// pushes that wait back, so a transfer that is still moving is never cut off.
    pub(super) async fn keep(
        &mut self,
        limit: Option<u32>,
        roots: Vec<Hash>,
        events: &tokio::sync::mpsc::UnboundedSender<crate::engine::Event>,
    ) -> Result<ServeEnd, Error> {
        let roots: BTreeSet<Hash> = roots.into_iter().collect();
        let limited = limit.is_some();
        // Every content connection still open, with the endpoint it belongs to, so the wait for a
        // holder's close never counts a refused connection as one.
        let mut open: BTreeMap<u64, [u8; 32]> = BTreeMap::new();
        let mut grace: Option<Grace> = None;
        if limited && self.places_full() && !self.holder_open(&open) {
            return Ok(ServeEnd::Downloaded);
        }
        loop {
            let left = self.expiry_ms_left();
            if left == 0 && self.expiry.is_some() {
                return Ok(ServeEnd::Expired);
            }
            let deadline = tokio::time::sleep(Duration::from_millis(left));
            tokio::pin!(deadline);
            tokio::select! {
                _ = tokio::signal::ctrl_c() => return Ok(ServeEnd::Interrupted),
                _ = wait_grace(&mut grace) => {
                    let left = self.idle_left();
                    if left.is_zero() {
                        return Ok(ServeEnd::Downloaded);
                    }
                    grace = Some(Box::pin(tokio::time::sleep(left)));
                }
                reported = self.receivers.recv() => {
                    let Some(reported) = reported else {
                        return Err(reports_stopped());
                    };
                    if let Some(end) = self.on_report(reported, &roots, limited, &mut open, &mut grace, events) {
                        return Ok(end);
                    }
                }
                _ = &mut deadline => {
                    // The timer is not the clock: a sleep can end early across a clock change.
                    if self.expiry.is_none_or(|expiry| now_ms() >= expiry) {
                        return Ok(ServeEnd::Expired);
                    }
                }
            }
        }
    }

    /// What one receiver report does to the wait: notes the connection, prints its line, and says
    /// whether the send is now done. A limited send is done once every place is taken and no
    /// connection of a holder is left; until then its grace runs from the holder's last activity.
    #[allow(clippy::too_many_arguments)]
    fn on_report(
        &self,
        reported: endpoint::Receiver,
        roots: &BTreeSet<Hash>,
        limited: bool,
        open: &mut BTreeMap<u64, [u8; 32]>,
        grace: &mut Option<Grace>,
        events: &tokio::sync::mpsc::UnboundedSender<crate::engine::Event>,
    ) -> Option<ServeEnd> {
        match &reported {
            endpoint::Receiver::Opened {
                connection,
                endpoint,
                ..
            } => {
                open.insert(*connection, *endpoint);
            }
            endpoint::Receiver::Closed { connection, .. } => {
                open.remove(connection);
            }
            _ => {}
        }
        self.report(reported, limited, roots, events);
        if !limited || !self.places_full() {
            return None;
        }
        if !self.holder_open(open) {
            return Some(ServeEnd::Downloaded);
        }
        if grace.is_none() {
            *grace = Some(Box::pin(tokio::time::sleep(GRACE)));
        }
        None
    }

    /// Whether every place a limited send offers is taken.
    fn places_full(&self) -> bool {
        lock_places(&self.places).full()
    }

    /// Whether a connection a place holder opened is still open.
    fn holder_open(&self, open: &BTreeMap<u64, [u8; 32]>) -> bool {
        let places = lock_places(&self.places);
        open.values().any(|endpoint| places.holds(endpoint))
    }

    /// How long a holder that is still connected may go without asking for or receiving anything
    /// before the send stops waiting for it to close.
    fn idle_left(&self) -> Duration {
        let last = lock_places(&self.places).last_active_ms();
        let grace_ms = GRACE.as_millis() as u64;
        Duration::from_millis(last.saturating_add(grace_ms).saturating_sub(now_ms()))
    }
}
