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
        let mut held = Held::default();
        let mut grace: Option<Grace> = None;
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
                    if let Some(end) = self.on_report(reported, &roots, limit, &mut held, &mut grace, events) {
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
    /// whether the send is now done. A limited send is done once the reports it has read show every
    /// place taken and no connection of a holder left; until then its grace runs from the holder's
    /// last activity.
    #[allow(clippy::too_many_arguments)]
    fn on_report(
        &self,
        reported: endpoint::Receiver,
        roots: &BTreeSet<Hash>,
        limit: Option<u32>,
        held: &mut Held,
        grace: &mut Option<Grace>,
        events: &tokio::sync::mpsc::UnboundedSender<crate::engine::Event>,
    ) -> Option<ServeEnd> {
        held.note(&reported);
        self.report(reported, limit.is_some(), roots, events);
        let limit = limit?;
        if let Some(end) = held.end(limit) {
            return Some(end);
        }
        if held.full(limit) && grace.is_none() {
            *grace = Some(Box::pin(tokio::time::sleep(GRACE)));
        }
        None
    }

    /// Milliseconds until the ticket deadline, or the far future when the ticket has none. A wait
    /// of its own reads the clock rather than trusting a timer across a clock change.
    pub(super) fn expiry_ms_left(&self) -> u64 {
        match self.expiry {
            Some(expiry) => expiry.saturating_sub(now_ms()),
            None => 100 * 365 * 24 * 60 * 60 * 1000,
        }
    }

    /// How long a holder that is still connected may go without asking for or receiving anything
    /// before the send stops waiting for it to close.
    fn idle_left(&self) -> Duration {
        let last = lock_places(&self.places).last_active_ms();
        let grace_ms = GRACE.as_millis() as u64;
        Duration::from_millis(last.saturating_add(grace_ms).saturating_sub(now_ms()))
    }
}

/// What the wait has read from its reports: the endpoints whose place it saw taken and the
/// connections still open, with the endpoint each belongs to. The shared places run ahead of the
/// channel: the policy takes a place while it answers a request, and the report of the connection
/// that asked is only read later. A wait that counted the shared places could therefore see the
/// send full before it read the open of the endpoint that filled it, and stop under a receiver.
#[derive(Debug, Default)]
struct Held {
    /// The endpoints whose place the wait has read a report of.
    placed: BTreeSet<[u8; 32]>,
    /// Every content connection still open, with the endpoint it belongs to, so no refused
    /// connection of an endpoint without a place counts as a holder.
    open: BTreeMap<u64, [u8; 32]>,
}

impl Held {
    /// Notes one report, before the wait decides anything from it.
    fn note(&mut self, reported: &endpoint::Receiver) {
        match reported {
            endpoint::Receiver::Opened {
                connection,
                endpoint,
                ..
            } => {
                self.open.insert(*connection, *endpoint);
            }
            endpoint::Receiver::Placed { endpoint, .. } => {
                self.placed.insert(*endpoint);
            }
            endpoint::Receiver::Closed { connection, .. } => {
                self.open.remove(connection);
            }
            endpoint::Receiver::Refused { .. } => {}
        }
    }

    /// Whether every place is taken, counted from the reports read.
    fn full(&self, limit: u32) -> bool {
        self.placed.len() >= limit as usize
    }

    /// Whether a connection of an endpoint whose place the wait has read is still open.
    fn holder_open(&self) -> bool {
        self.open
            .values()
            .any(|endpoint| self.placed.contains(endpoint))
    }

    /// The end this view says the send reached: every place taken and no holder left.
    fn end(&self, limit: u32) -> Option<ServeEnd> {
        (self.full(limit) && !self.holder_open()).then_some(ServeEnd::Downloaded)
    }
}

#[cfg(test)]
mod tests {
    use super::{Held, ServeEnd};
    use crate::engine::endpoint::Receiver;
    use crate::engine::policy::{Admission, Places};

    fn opened(connection: u64, endpoint: [u8; 32]) -> Receiver {
        Receiver::Opened {
            connection,
            endpoint,
            relayed: false,
        }
    }

    fn placed(endpoint: [u8; 32]) -> Receiver {
        Receiver::Placed {
            endpoint,
            relayed: false,
        }
    }

    fn closed(connection: u64, endpoint: [u8; 32]) -> Receiver {
        Receiver::Closed {
            connection,
            endpoint,
            relayed: false,
            claimed_whole: false,
        }
    }

    /// The wait decides from the reports it has read. The policy takes B's place while it answers
    /// B's request, so the shared places hold A and B before the report of B's connection is read.
    /// A wait that counted them would see the send full, read A's close, find no holder open and
    /// stop under B; this one has read only A's place, so it keeps waiting.
    #[test]
    fn a_close_read_before_the_place_that_filled_the_send_does_not_end_it() {
        let a = [b'a'; 32];
        let b = [b'b'; 32];
        let mut places = Places::default();
        places.set_limit(Some(2));
        assert_eq!(places.admit(a, 1), Admission::Took);
        assert_eq!(places.admit(b, 2), Admission::Took);
        assert!(places.full(), "the policy did not hold both places");

        let mut held = Held::default();
        for report in [opened(1, a), placed(a), closed(1, a)] {
            held.note(&report);
        }
        assert_eq!(held.end(2), None, "the wait ended before B's place");

        for report in [opened(2, b), placed(b)] {
            held.note(&report);
        }
        assert_eq!(held.end(2), None, "B's connection was still open");

        held.note(&closed(2, b));
        assert_eq!(held.end(2), Some(ServeEnd::Downloaded));
    }
}
