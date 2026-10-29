//! The N places a limited send offers. A place belongs to an endpoint id and is taken by the first
//! request the policy lets in from it, whatever that request asks for and however the receiver's
//! connection ends. The place is the bound: once all N are taken, every other endpoint id is refused
//! before it can read anything.

use std::collections::BTreeSet;

/// How many refused endpoint ids a send names. A ticket holder who is refused can open as many
/// connections as it likes, and the terminal belongs to the user: after this many lines the sender
/// refuses in silence. The cap counts distinct endpoints, not requests, and the same endpoint is
/// never named twice, so a flood of connections from one host costs one line.
const REFUSAL_LINES: usize = 3;

/// What admitting one request did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Admission {
    /// Served, and the endpoint already held a place or the send has no limit.
    Served,
    /// Served, and this request took the endpoint's place.
    Took,
    /// Refused, and the line about it is owed.
    Refused,
    /// Refused, and the line was said already or the cap has been reached.
    Unreported,
}

/// The places a send offers and the activity that keeps an idle holder from ending it early.
#[derive(Debug, Default)]
pub(crate) struct Places {
    /// How many endpoint ids may hold a place, or none for a send that does not count.
    limit: Option<usize>,
    /// The endpoint ids that hold a place, in the order they took one.
    holders: BTreeSet<[u8; 32]>,
    /// The endpoint ids a refusal line has been made for.
    refused: BTreeSet<[u8; 32]>,
    /// How many refusal lines have been made.
    lines: usize,
    /// When a holder last asked for or received something, in the clock [`super::now_ms`] reads.
    last_active_ms: u64,
}

impl Places {
    /// Sets how many places the send offers. The serve loop sets it before it starts, and a send
    /// that is not counting leaves it at none.
    pub(crate) fn set_limit(&mut self, limit: Option<u32>) {
        self.limit = limit.map(|limit| limit as usize);
    }

    /// How many places the send offers, or `None` when it does not count.
    pub(crate) fn limit(&self) -> Option<usize> {
        self.limit
    }

    /// Whether `endpoint` holds a place.
    pub(crate) fn holds(&self, endpoint: &[u8; 32]) -> bool {
        self.holders.contains(endpoint)
    }

    /// Whether every place is taken. The policy's view runs ahead of the wait's, which counts the
    /// reports it has read instead, so only a test asks this.
    #[cfg(test)]
    pub(crate) fn full(&self) -> bool {
        self.limit.is_some_and(|limit| self.holders.len() >= limit)
    }

    /// Admits a request from `endpoint`, taking its place the first time. A send with no limit
    /// admits everything. `now` is when the request arrived, so a holder's activity can be read
    /// without another clock.
    pub(crate) fn admit(&mut self, endpoint: [u8; 32], now: u64) -> Admission {
        let Some(limit) = self.limit else {
            return Admission::Served;
        };
        if self.holders.contains(&endpoint) {
            self.last_active_ms = now;
            return Admission::Served;
        }
        if self.holders.len() < limit {
            self.holders.insert(endpoint);
            self.last_active_ms = now;
            return Admission::Took;
        }
        if !self.refused.insert(endpoint) {
            return Admission::Unreported;
        }
        if self.lines >= REFUSAL_LINES {
            return Admission::Unreported;
        }
        self.lines += 1;
        Admission::Refused
    }

    /// Notes that a holder asked for or received something, so a transfer that is still moving is
    /// not taken for a connection that does nothing.
    pub(crate) fn touch(&mut self, endpoint: &[u8; 32], now: u64) {
        if self.holders.contains(endpoint) {
            self.last_active_ms = now;
        }
    }

    /// When a holder last did something.
    pub(crate) fn last_active_ms(&self) -> u64 {
        self.last_active_ms
    }
}

#[cfg(test)]
mod tests {
    use super::{Admission, Places, REFUSAL_LINES};

    fn places(limit: u32) -> Places {
        let mut places = Places::default();
        places.set_limit(Some(limit));
        places
    }

    /// A place is spent the first time an endpoint is let in, and every later request from it is
    /// served without spending another.
    #[test]
    fn a_place_is_spent_once_per_endpoint() {
        let mut places = places(2);
        assert_eq!(places.admit([1; 32], 1), Admission::Took);
        assert_eq!(places.admit([1; 32], 2), Admission::Served);
        assert!(!places.full());
        assert_eq!(places.admit([2; 32], 3), Admission::Took);
        assert!(places.full());
        assert!(places.holds(&[1; 32]));
    }

    /// Once every place is taken, an endpoint that holds none is refused, and its line is owed once
    /// however many requests it makes.
    #[test]
    fn a_refused_endpoint_is_named_once() {
        let mut places = places(1);
        assert_eq!(places.admit([1; 32], 1), Admission::Took);
        assert_eq!(places.admit([2; 32], 2), Admission::Refused);
        assert_eq!(places.admit([2; 32], 3), Admission::Unreported);
        assert_eq!(places.admit([3; 32], 4), Admission::Refused);
        assert!(!places.holds(&[2; 32]));
    }

    /// A flood of refused endpoints costs the terminal the cap and no more.
    #[test]
    fn refusals_are_capped() {
        let mut places = places(1);
        places.admit([1; 32], 1);
        let lines = (2..2 + REFUSAL_LINES as u8 + 4)
            .filter(|byte| places.admit([*byte; 32], 2) == Admission::Refused)
            .count();
        assert_eq!(lines, REFUSAL_LINES);
    }

    /// A send with no limit counts nobody and refuses nobody, which is what plain `--keep` was.
    #[test]
    fn a_send_without_a_limit_admits_everything() {
        let mut places = Places::default();
        for byte in 1..10 {
            assert_eq!(places.admit([byte; 32], 1), Admission::Served);
        }
        assert!(!places.full());
    }

    /// Activity is only a holder's: a refused endpoint cannot keep a send alive by asking.
    #[test]
    fn only_a_holders_activity_counts() {
        let mut places = places(1);
        places.admit([1; 32], 10);
        places.touch(&[2; 32], 20);
        assert_eq!(places.last_active_ms(), 10);
        places.touch(&[1; 32], 30);
        assert_eq!(places.last_active_ms(), 30);
    }
}
