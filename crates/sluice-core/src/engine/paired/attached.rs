//! The place a send's content handlers go on the daemon's endpoint, and the two protocol handlers
//! that forward to them while the send is attached.

use std::{
    io,
    sync::{Arc, Mutex},
};

use iroh::{
    Endpoint,
    endpoint::Connection,
    protocol::{AcceptError, ProtocolHandler},
};
use iroh_blobs::BlobsProtocol;
use n0_future::task::AbortOnDropHandle;
use tokio::sync::{OwnedMutexGuard, broadcast, mpsc::UnboundedSender};

use super::super::SluiceProtocol;
use super::super::conn::ConnIds;
use super::super::endpoint::Receiver;
use super::{Listener, listener_for};
use crate::{Error, Home};
impl Listener {
    /// The place a send's content handlers go, with its reports forwarded to `to`.
    async fn attach(&self, to: UnboundedSender<Receiver>) -> Attached {
        let guard = Arc::clone(&self.send_lock).lock_owned().await;
        let mut reports = self.reports.subscribe();
        let forward = AbortOnDropHandle::new(n0_future::task::spawn(async move {
            loop {
                match reports.recv().await {
                    Ok(report) => {
                        if to.send(report).is_err() {
                            break;
                        }
                    }
                    // A send that falls behind loses the oldest reports. The next connection's
                    // open is still delivered, and a report for one that closed before the send
                    // attached means nothing to it.
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        }));
        Attached {
            _guard: guard,
            endpoint: self.endpoint.clone(),
            slots: self.slots.clone(),
            ids: Arc::clone(&self.ids),
            _forward: forward,
        }
    }
}

/// A send's claim on the daemon's endpoint, and the place its protocols go.
pub struct Attached {
    _guard: OwnedMutexGuard<()>,
    endpoint: Endpoint,
    slots: Slots,
    ids: Arc<ConnIds>,
    /// Reads this send's copy of the endpoint's connection reports, stopped with the claim.
    _forward: AbortOnDropHandle<()>,
}

impl Attached {
    /// The endpoint a send attaches to.
    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    /// The connection ids of the daemon's endpoint, which the send's policy reads so a request is
    /// credited to the connection the daemon's handshake gave an id.
    pub(crate) fn ids(&self) -> Arc<ConnIds> {
        Arc::clone(&self.ids)
    }

    /// Installs the two content handlers for as long as this value lives, and refuses when another
    /// send already has them.
    pub fn install(&self, blobs: BlobsProtocol, sluice: SluiceProtocol) -> Result<(), Error> {
        self.slots.install(blobs, sluice)
    }
}

impl Drop for Attached {
    fn drop(&mut self) {
        self.slots.clear();
    }
}

/// A send started for a home with a registered listener attaches to it, so one endpoint id answers
/// for the daemon. Every other send binds its own endpoint, as it always did. The reports go to
/// `to`, which is the send's own channel of receiver events.
pub(in crate::engine) async fn attach_for(
    home: &Home,
    to: UnboundedSender<Receiver>,
) -> Result<Option<Attached>, Error> {
    match listener_for(home) {
        Some(listener) => Ok(Some(listener.attach(to).await)),
        None => Ok(None),
    }
}

/// The two content protocols a running send installs on the daemon's endpoint.
#[derive(Clone, Default)]
pub(super) struct Slots {
    blobs: Arc<Mutex<Option<BlobsProtocol>>>,
    sluice: Arc<Mutex<Option<SluiceProtocol>>>,
}

impl Slots {
    fn install(&self, blobs: BlobsProtocol, sluice: SluiceProtocol) -> Result<(), Error> {
        let mut slot = lock(&self.blobs);
        if slot.is_some() {
            return Err(Error::Storage(
                "another send is already served by the daemon's endpoint".to_string(),
            ));
        }
        *slot = Some(blobs);
        *lock(&self.sluice) = Some(sluice);
        Ok(())
    }

    fn clear(&self) {
        *lock(&self.blobs) = None;
        *lock(&self.sluice) = None;
    }
}

/// `/iroh-bytes/4` on the daemon's endpoint: the running send's handler, or a close.
pub(super) struct AttachedBlobs(pub(super) Slots);

impl std::fmt::Debug for AttachedBlobs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AttachedBlobs").finish_non_exhaustive()
    }
}

impl ProtocolHandler for AttachedBlobs {
    async fn accept(&self, conn: Connection) -> Result<(), AcceptError> {
        let handler = lock(&self.0.blobs).clone();
        match handler {
            Some(handler) => handler.accept(conn).await,
            None => Err(no_send()),
        }
    }
}

/// `sluice/1` on the daemon's endpoint: the running send's handler, or a close.
pub(super) struct AttachedSluice(pub(super) Slots);

impl std::fmt::Debug for AttachedSluice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AttachedSluice").finish_non_exhaustive()
    }
}

impl ProtocolHandler for AttachedSluice {
    async fn accept(&self, conn: Connection) -> Result<(), AcceptError> {
        let handler = lock(&self.0.sluice).clone();
        match handler {
            Some(handler) => handler.accept(conn).await,
            None => Err(no_send()),
        }
    }
}

fn no_send() -> AcceptError {
    AcceptError::from_err(io::Error::new(
        io::ErrorKind::NotConnected,
        "the daemon has no send running",
    ))
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
