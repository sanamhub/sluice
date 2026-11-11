//! What each connection asked for and was allowed to read. A completion code is believed only from
//! a connection whose set holds every root of the transfer, and a receipt is answered only to the
//! endpoint that asked for the root it is about. The sets have to survive as long as the wait does.

use std::collections::{BTreeMap, BTreeSet};

use iroh_blobs::Hash;

use crate::engine::conn::Peer;

/// One connection: the endpoint that made it and the roots it asked for and was allowed to read.
#[derive(Debug)]
struct Record {
    peer: Peer,
    roots: BTreeSet<Hash>,
}

/// The roots each connection asked for and was allowed to read.
#[derive(Debug, Default)]
pub(crate) struct Asked {
    /// Keyed by the id [`super::conn::ConnIds`] gave the connection, which only grows, so the
    /// lowest key is the connection that asked first.
    by: BTreeMap<u64, Record>,
}

/// How many connections the provider remembers. Past this the oldest record goes, so a peer that
/// opens thousands of connections can only make the sender serve longer, never shorter. A receipt
/// asked for after such a flood may find no record of its ask and be refused, which is an absence
/// and not a wrong answer.
const MAX_ASKED: usize = 1024;

impl Asked {
    /// Notes that `connection`, made by `peer`, asked for `hash`. A full map loses the record of
    /// the connection that asked first: ids grow with time, so that is the oldest insertion and not
    /// merely the lowest number a peer happened to land on.
    pub(crate) fn remember(&mut self, connection: u64, peer: Peer, hash: Hash) {
        if self.by.len() >= MAX_ASKED
            && !self.by.contains_key(&connection)
            && let Some(oldest) = self.by.keys().next().copied()
        {
            self.by.remove(&oldest);
        }
        self.by
            .entry(connection)
            .or_insert_with(|| Record {
                peer,
                roots: BTreeSet::new(),
            })
            .roots
            .insert(hash);
    }

    /// The roots `connection` asked for and was allowed to read.
    pub(crate) fn of(&self, connection: u64) -> BTreeSet<Hash> {
        self.by
            .get(&connection)
            .map(|record| record.roots.clone())
            .unwrap_or_default()
    }

    /// Whether `peer` asked for `hash` on a connection the policy let in. A receipt is answered on
    /// that record and not on the asking connection's, because the ask travels on a connection of
    /// its own which never asks for content.
    pub(crate) fn peer_asked(&self, peer: &[u8; 32], hash: Hash) -> bool {
        self.by
            .values()
            .any(|record| &record.peer.endpoint == peer && record.roots.contains(&hash))
    }
}

#[cfg(test)]
mod tests {
    use super::{Asked, MAX_ASKED};
    use crate::engine::conn::{ConnIds, Peer};
    use iroh_blobs::Hash;

    fn peer(byte: u8) -> Peer {
        Peer {
            endpoint: [byte; 32],
            relayed: false,
        }
    }

    /// A peer with no ticket that lands on a closed connection's iroh id must not inherit the
    /// record of the connection it replaced: it would otherwise have its completion code believed
    /// for roots it never asked for.
    #[test]
    fn a_connection_that_reuses_an_id_inherits_nothing() {
        let ids = ConnIds::default();
        let mut asked = Asked::default();
        let root = Hash::from([1u8; 32]);
        let first = ids.assign(9, peer(1));
        asked.remember(first, peer(1), root);
        assert!(asked.of(first).contains(&root));
        ids.closed(9, first);
        let second = ids.assign(9, peer(2));
        assert_ne!(first, second);
        assert!(asked.of(second).is_empty());
    }

    /// The cap drops the connection that asked first. Ids grow with time, so that is the lowest
    /// key and not whichever id a peer happened to land on.
    #[test]
    fn the_cap_evicts_the_oldest_connection() {
        let mut asked = Asked::default();
        let root = Hash::from([2u8; 32]);
        for connection in 1..=MAX_ASKED as u64 {
            asked.remember(connection, peer(1), root);
        }
        assert_eq!(asked.by.len(), MAX_ASKED);
        asked.remember(MAX_ASKED as u64 + 1, peer(1), root);
        assert_eq!(asked.by.len(), MAX_ASKED);
        assert!(asked.of(1).is_empty(), "the first connection was kept");
        assert!(!asked.of(MAX_ASKED as u64 + 1).is_empty());
        assert!(!asked.of(2).is_empty());
    }

    /// A receipt is answered on the endpoint's record, not on the connection the ask arrives on, so
    /// the same endpoint asking twice on two connections is the same asker.
    #[test]
    fn a_peer_is_known_by_any_connection_that_asked_for_the_root() {
        let mut asked = Asked::default();
        let root = Hash::from([3u8; 32]);
        let other = Hash::from([4u8; 32]);
        asked.remember(1, peer(1), root);
        asked.remember(2, peer(2), root);
        assert!(asked.peer_asked(&[1; 32], root));
        assert!(asked.peer_asked(&[2; 32], root));
        assert!(!asked.peer_asked(&[3; 32], root));
        assert!(!asked.peer_asked(&[1; 32], other));
    }
}
