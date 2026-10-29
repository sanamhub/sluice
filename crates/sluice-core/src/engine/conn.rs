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

/// What this process remembers about the connections it accepted: the id it gave each one, keyed by
/// the id iroh reports for it. Both the handshake that assigns an id and the policy that checks a
/// completion code against it read the same map.
#[derive(Debug, Default)]
pub struct ConnIds {
    /// The last id handed out. The first one is 1, so no connection is ever id 0.
    next: AtomicU64,
    /// iroh's id of each live connection, mapped to this process's id for it.
    live: Mutex<BTreeMap<u64, u64>>,
}

impl ConnIds {
    /// Gives the connection iroh calls `stable` an id of this process's own. Called at the
    /// handshake, which happens before the connection can send a request.
    pub fn assign(&self, stable: u64) -> u64 {
        // Relaxed is enough: the map's own lock orders this against every reader.
        let own = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        lock(&self.live).insert(stable, own);
        own
    }

    /// The id this process gave the connection iroh calls `stable`, if it has one. `None` is a
    /// connection this process never saw a handshake for, whose requests cannot be counted towards
    /// a completion.
    pub fn own(&self, stable: u64) -> Option<u64> {
        lock(&self.live).get(&stable).copied()
    }

    /// Forgets a connection that has closed. The entry is removed only while it is still the
    /// closing connection's own: iroh may already have given the same id to the next connection,
    /// and that connection's record has to stay.
    pub fn closed(&self, stable: u64, own: u64) {
        let mut live = lock(&self.live);
        if live.get(&stable) == Some(&own) {
            live.remove(&stable);
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
    use super::ConnIds;

    /// iroh's id for a closed connection is handed to the next one, so a new connection must never
    /// take over the record of the one it replaced.
    #[test]
    fn a_reused_id_belongs_to_a_new_connection() {
        let ids = ConnIds::default();
        let first = ids.assign(7);
        assert_eq!(ids.own(7), Some(first));
        ids.closed(7, first);
        assert_eq!(ids.own(7), None);
        let second = ids.assign(7);
        assert_ne!(first, second);
        assert_eq!(ids.own(7), Some(second));
        assert_ne!(first, 0);
    }

    /// The close of the old connection can be handled after the new one has its id, so forgetting
    /// checks whose record it is before removing anything.
    #[test]
    fn a_late_close_does_not_forget_the_connection_that_replaced_it() {
        let ids = ConnIds::default();
        let first = ids.assign(3);
        let second = ids.assign(3);
        ids.closed(3, first);
        assert_eq!(ids.own(3), Some(second));
    }
}
