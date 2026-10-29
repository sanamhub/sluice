//! The ids this process gives its connections. iroh's own id is an address that is unique only
//! while the connection lives: the allocator hands a closed connection's id to a later one, so a
//! record kept under it would be inherited by a connection that never asked for anything. These
//! ids only grow, which makes every one of them belong to exactly one connection in this process.

use std::{
    collections::BTreeMap,
    sync::{
        Mutex, MutexGuard,
        atomic::{AtomicU64, Ordering},
    },
};

/// The peer a content connection belongs to: the endpoint id the ticket dialled and the endpoint
/// this process sees, plus how the connection reaches it. A receiver that reconnects under the same
/// endpoint id is the same peer, which is why a limited send counts places by this value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Peer {
    /// The peer's endpoint id, as the bytes a ticket spells in hex.
    pub endpoint: [u8; 32],
    /// Whether no path of the connection is a direct one.
    pub relayed: bool,
}

/// What this process remembers about the connections it accepted: the id it gave each one, keyed by
/// the id iroh reports for it, and the peer of each id it handed out. Both the handshake that
/// assigns an id and the policy that checks a completion code against it read the same map.
#[derive(Debug, Default)]
pub struct ConnIds {
    /// The last id handed out. The first one is 1, so no connection is ever id 0.
    next: AtomicU64,
    /// The live connections, and the id and peer each one was given.
    live: Mutex<Live>,
}

#[derive(Debug, Default)]
struct Live {
    /// iroh's id of each live connection, mapped to this process's id for it and its peer.
    by_stable: BTreeMap<u64, (u64, Peer)>,
    /// This process's id of each live connection, mapped to its peer, so a request that names only
    /// the id the policy knows can find who made it.
    by_own: BTreeMap<u64, Peer>,
}

impl ConnIds {
    /// Gives the connection iroh calls `stable` an id of this process's own. Called at the
    /// handshake, which happens before the connection can send a request.
    pub fn assign(&self, stable: u64, peer: Peer) -> u64 {
        // Relaxed is enough: the map's own lock orders this against every reader.
        let own = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        let mut live = lock(&self.live);
        live.by_stable.insert(stable, (own, peer));
        live.by_own.insert(own, peer);
        own
    }

    /// The id this process gave the connection iroh calls `stable`, if it has one. `None` is a
    /// connection this process never saw a handshake for, whose requests cannot be counted towards
    /// a completion or a place.
    pub fn own(&self, stable: u64) -> Option<u64> {
        lock(&self.live).by_stable.get(&stable).map(|(own, _)| *own)
    }

    /// The peer of the connection this process calls `own`.
    pub fn peer(&self, own: u64) -> Option<Peer> {
        lock(&self.live).by_own.get(&own).copied()
    }

    /// Forgets a connection that has closed. The entry is removed only while it is still the
    /// closing connection's own: iroh may already have given the same id to the next connection,
    /// and that connection's record has to stay.
    pub fn closed(&self, stable: u64, own: u64) {
        let mut live = lock(&self.live);
        if live.by_stable.get(&stable).map(|(id, _)| *id) == Some(own) {
            live.by_stable.remove(&stable);
            live.by_own.remove(&own);
        }
    }
}

/// The lock is held only while a map is read or edited, so a panic in one task poisoning it changes
/// nothing here.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::{ConnIds, Peer};

    fn peer(byte: u8) -> Peer {
        Peer {
            endpoint: [byte; 32],
            relayed: false,
        }
    }

    /// iroh's id for a closed connection is handed to the next one, so a new connection must never
    /// take over the record of the one it replaced.
    #[test]
    fn a_reused_id_belongs_to_a_new_connection() {
        let ids = ConnIds::default();
        let first = ids.assign(7, peer(1));
        assert_eq!(ids.own(7), Some(first));
        ids.closed(7, first);
        assert_eq!(ids.own(7), None);
        let second = ids.assign(7, peer(2));
        assert_ne!(first, second);
        assert_eq!(ids.own(7), Some(second));
        assert_ne!(first, 0);
        assert_eq!(ids.peer(second), Some(peer(2)));
        assert_eq!(ids.peer(first), None);
    }

    /// The close of the old connection can be handled after the new one has its id, so forgetting
    /// checks whose record it is before removing anything.
    #[test]
    fn a_late_close_does_not_forget_the_connection_that_replaced_it() {
        let ids = ConnIds::default();
        let first = ids.assign(3, peer(1));
        let second = ids.assign(3, peer(2));
        ids.closed(3, first);
        assert_eq!(ids.own(3), Some(second));
        assert_eq!(ids.peer(second), Some(peer(2)));
    }
}
