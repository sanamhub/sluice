//! Pairing, `SPEC.md` section 11.2: one stream, 32 bytes in, one status byte out.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use iroh::{
    endpoint::{Connection, RecvStream},
    protocol::{AcceptError, ProtocolHandler},
};
use subtle::ConstantTimeEq;

use super::limits::{Held, Limits, lock};
use crate::device::{Devices, Grant};
use crate::engine::now_ms;

/// How long a pairing ticket lives unless `--ttl` says otherwise.
pub(super) const DEFAULT_TTL: u64 = 600;
/// The longest `--ttl` a pairing ticket may be given.
pub(super) const MAX_TTL: u64 = 3600;

/// How long the daemon waits for the 32 bytes before it answers `0x01`.
const READ_WAIT: Duration = Duration::from_secs(10);
/// How long the handler waits for the client to close after the status byte, so the byte is not
/// cut off by a close that races it.
const CLOSE_WAIT: Duration = Duration::from_secs(5);

/// The tickets this daemon has made, has not forgotten and has not expired, and the grants they
/// made.
pub(super) struct Pairing {
    devices: Devices,
    tickets: Mutex<HashMap<[u8; 32], Pending>>,
}

struct Pending {
    grant: Grant,
    expires_ms: u64,
    /// The label the ticket's maker gave the device, if it gave one.
    label: Option<String>,
    /// The endpoint id that redeemed it, once one has.
    redeemed_by: Option<[u8; 32]>,
}

/// What a pairing attempt was answered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Redeemed {
    /// `0x00`: the grant is written.
    Accepted,
    /// `0x01`: no ticket matched, it expired, another endpoint redeemed it, or it was revoked.
    Refused,
    /// `0x02`: the grants file could not be read or written.
    Unreadable,
}

impl Redeemed {
    pub(super) fn status(self) -> u8 {
        match self {
            Self::Accepted => 0x00,
            Self::Refused => 0x01,
            Self::Unreadable => 0x02,
        }
    }
}

impl Pairing {
    pub(super) fn new(devices: Devices) -> Self {
        Self {
            devices,
            tickets: Mutex::new(HashMap::new()),
        }
    }

    /// Remembers a ticket's secret until it expires or its grant is revoked.
    pub(super) fn remember(
        &self,
        secret: [u8; 32],
        grant: Grant,
        expires_ms: u64,
        label: Option<String>,
    ) {
        lock(&self.tickets).insert(
            secret,
            Pending {
                grant,
                expires_ms,
                label,
                redeemed_by: None,
            },
        );
    }

    /// Answers the 32 bytes a peer wrote. The secret is compared in constant time with every
    /// ticket. A ticket that is expired is forgotten, and one that the same endpoint id redeemed
    /// before answers success again while its grant is still there.
    pub(super) fn redeem(&self, peer: [u8; 32], secret: &[u8]) -> Redeemed {
        let mut tickets = lock(&self.tickets);
        let mut found = None;
        for candidate in tickets.keys() {
            if candidate.ct_eq(secret).into() {
                found = Some(*candidate);
            }
        }
        let Some(key) = found else {
            return Redeemed::Refused;
        };
        let now = now_ms();
        let pending = tickets.get(&key).expect("the key came from this map");
        if pending.expires_ms <= now {
            tickets.remove(&key);
            return Redeemed::Refused;
        }
        match pending.redeemed_by {
            Some(by) if by == peer => return Redeemed::Accepted,
            Some(_) => return Redeemed::Refused,
            None => {}
        }
        let grant = pending.grant;
        let label = pending.label.clone();
        // The lock is held across the write so two peers that present one secret at the same time
        // cannot both be granted it.
        match self.devices.grant_to(peer, grant, now, label.as_deref()) {
            Ok(_) => {
                if let Some(pending) = tickets.get_mut(&key) {
                    pending.redeemed_by = Some(peer);
                }
                Redeemed::Accepted
            }
            Err(_) => Redeemed::Unreadable,
        }
    }

    /// Forgets the ticket that made `peer`'s grant, so a retry after a revoke is refused.
    pub(super) fn forget(&self, peer: [u8; 32]) {
        lock(&self.tickets).retain(|_, pending| pending.redeemed_by != Some(peer));
    }
}

/// One pairing listener on `/sluice/pair/1`.
pub(super) struct PairHandler {
    pub(super) pairing: Arc<Pairing>,
    pub(super) limits: Limits,
    pub(super) held: Held,
}

impl std::fmt::Debug for PairHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PairHandler").finish_non_exhaustive()
    }
}

impl ProtocolHandler for PairHandler {
    async fn accept(&self, conn: Connection) -> Result<(), AcceptError> {
        let peer = *conn.remote_id().as_bytes();
        let Some(lease) = self.limits.acquire(peer) else {
            // The limit is the answer, and a stream is not opened for it.
            conn.close(
                iroh::endpoint::VarInt::from_u32(0),
                b"too many pairing streams",
            );
            return Ok(());
        };
        let held = self.held.hold(&conn);
        self.answer(&conn, peer).await;
        // Wait for the client to close, so a close that races the status byte does not cut it off.
        let _ = tokio::time::timeout(CLOSE_WAIT, conn.closed()).await;
        drop(held);
        drop(lease);
        Ok(())
    }
}

impl PairHandler {
    /// Reads the secret, answers one status byte, and returns. A read that fails or does not end
    /// with exactly 32 bytes is a refusal, and the answer is written either way.
    async fn answer(&self, conn: &Connection, peer: [u8; 32]) {
        let Ok((mut send, mut recv)) = conn.accept_bi().await else {
            return;
        };
        let redeeming = tokio::time::timeout(READ_WAIT, read_secret(&mut recv)).await;
        let status = match redeeming {
            Ok(Ok(secret)) => self.pairing.redeem(peer, &secret).status(),
            _ => Redeemed::Refused.status(),
        };
        let _ = send.write_all(&[status]).await;
        let _ = send.finish();
    }
}

/// The 32 bytes the peer wrote, or an error for a stream that wrote fewer or more.
async fn read_secret(recv: &mut RecvStream) -> Result<[u8; 32], ()> {
    // One byte past the credential, so a stream that writes 33 is refused rather than silently
    // read as the first 32.
    let bytes = recv.read_to_end(33).await.map_err(|_| ())?;
    bytes.try_into().map_err(|_| ())
}
