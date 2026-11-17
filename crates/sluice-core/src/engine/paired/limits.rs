//! The stream limits of `SPEC.md` section 11.2, and the connections a revoke closes.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use iroh::endpoint::{Connection, VarInt};

/// The most pairing streams one endpoint id may hold at once.
const PER_ENDPOINT: u32 = 4;
/// The most pairing streams the daemon may hold at once.
const TOTAL: u32 = 16;

/// The limits, shared by every pairing stream.
#[derive(Clone, Default)]
pub(super) struct Limits {
    inner: Arc<Mutex<Counts>>,
}

#[derive(Default)]
struct Counts {
    per: HashMap<[u8; 32], u32>,
    total: u32,
}

/// One stream's place. Dropping it gives the place back, so a stream that ends for any reason stops
/// counting.
pub(super) struct Lease {
    inner: Arc<Mutex<Counts>>,
    peer: [u8; 32],
}

impl Limits {
    /// Takes a place for a stream from `peer`, or `None` when it would pass a limit.
    pub(super) fn acquire(&self, peer: [u8; 32]) -> Option<Lease> {
        let mut counts = lock(&self.inner);
        let held = counts.per.get(&peer).copied().unwrap_or(0);
        if held >= PER_ENDPOINT || counts.total >= TOTAL {
            return None;
        }
        *counts.per.entry(peer).or_default() += 1;
        counts.total += 1;
        Some(Lease {
            inner: Arc::clone(&self.inner),
            peer,
        })
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        let mut counts = lock(&self.inner);
        if let Some(held) = counts.per.get_mut(&self.peer) {
            *held = held.saturating_sub(1);
            if *held == 0 {
                counts.per.remove(&self.peer);
            }
        }
        counts.total = counts.total.saturating_sub(1);
    }
}

/// The connections the daemon's endpoint has open, so a revoke closes the peer's.
#[derive(Clone, Default)]
pub(super) struct Held {
    inner: Arc<Mutex<HashMap<u64, Connection>>>,
}

/// One connection's place in [`Held`], removed when the handler ends.
pub(super) struct HeldGuard {
    inner: Arc<Mutex<HashMap<u64, Connection>>>,
    key: u64,
}

impl Held {
    /// Records `conn` for as long as the returned guard lives.
    pub(super) fn hold(&self, conn: &Connection) -> HeldGuard {
        let key = conn.stable_id() as u64;
        lock(&self.inner).insert(key, conn.clone());
        HeldGuard {
            inner: Arc::clone(&self.inner),
            key,
        }
    }

    /// Closes every connection `peer` holds.
    pub(super) fn close(&self, peer: [u8; 32]) {
        let mut held = lock(&self.inner);
        held.retain(|_, conn| {
            if *conn.remote_id().as_bytes() == peer {
                conn.close(VarInt::from_u32(0), b"revoked");
                false
            } else {
                true
            }
        });
    }
}

impl Drop for HeldGuard {
    fn drop(&mut self) {
        lock(&self.inner).remove(&self.key);
    }
}

/// A poisoned mutex holds a map that is still a map, so a panic in one handler changes nothing
/// here.
pub(super) fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
