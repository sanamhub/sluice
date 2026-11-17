//! The daemon's identity endpoint: one endpoint for the life of the daemon, the three ALPNs ADR 0039
//! adds, and the content protocols a send installs while it runs. `SPEC.md` sections 11.2 to 11.4.
//!
//! A send in this process attaches here instead of binding a second endpoint with the same key, so
//! one endpoint id answers for the daemon whether or not a send is running. The registration is by
//! home, so only a process that started a listener for that home attaches.

use std::{
    collections::HashMap,
    io,
    net::SocketAddr,
    path::PathBuf,
    sync::{
        Arc, Mutex, OnceLock, Weak,
        atomic::{AtomicBool, Ordering},
    },
};

use iroh::{
    Endpoint,
    endpoint::Connection,
    protocol::{AcceptError, ProtocolHandler, Router},
};
use iroh_blobs::BlobsProtocol;
use n0_future::task::AbortOnDropHandle;
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard, broadcast, mpsc::UnboundedSender};

use super::conn::ConnIds;
use super::endpoint::Receiver;
use super::paired::limits::{Held, Limits};
use super::paired::pair::{DEFAULT_TTL, MAX_TTL, PairHandler, Pairing};
use super::{
    ALPN, ALPN_BLOBS, ALPN_DELIVER, ALPN_PAIR, Congestion, RelayModeChoice, SluiceProtocol,
    endpoint, now_ms,
};
use crate::device::{Devices, Grant};
use crate::ticket::{Body, Ticket};
use crate::{Error, Home};
use deliver::{DeliverHandler, Deliverer, Inbox};

mod deliver;
mod limits;
mod pair;

/// How many connection reports the daemon holds for an attached send that has fallen behind. A
/// report is a few bytes and a send reads them as they arrive, so this is far more than it needs.
const REPORTS_BUFFER: usize = 256;

pub use deliver::Inbox as InboxFolder;

/// A pairing ticket, ready to print once.
pub struct PairTicket {
    /// The text to show. A secret: never written to a log.
    pub text: String,
    /// When it stops working, in UTC milliseconds since the Unix epoch.
    pub expires_ms: u64,
}

/// The daemon's long-lived endpoint.
pub struct Listener {
    dir: PathBuf,
    endpoint: Endpoint,
    _router: Router,
    devices: Devices,
    pairing: Arc<Pairing>,
    held: Held,
    /// Held while a send's handlers are installed, so two sends cannot share the content ALPNs.
    send_lock: Arc<AsyncMutex<()>>,
    slots: Slots,
    /// The ids of this endpoint's content connections, shared with every send that attaches: a
    /// request is credited to the connection the daemon's own handshake reported.
    ids: Arc<ConnIds>,
    /// Every content connection's open and close, to the sends that are attached now. A report
    /// with no subscriber is dropped, because no send can use a connection it is not serving.
    reports: broadcast::Sender<Receiver>,
    /// The task that reads the endpoint's reports and broadcasts them, stopped with the listener.
    _reports_task: AbortOnDropHandle<()>,
    stopped: AtomicBool,
}

impl Listener {
    /// Binds the daemon's endpoint from `$SLUICE_HOME/identity.key` and serves pairing and delivery
    /// on it. A delivery listener exists only when `inbox` is given.
    pub async fn bind(
        home: &Home,
        relay: RelayModeChoice,
        inbox: Option<Inbox>,
    ) -> Result<Arc<Self>, Error> {
        let seed = home.identity()?.secret_seed();
        let saved = home.saved_port();
        // The daemon's endpoint reports every content connection, and each attached send gets those
        // reports: a limited send places its receivers from them, and one send is attached at a
        // time, so every report belongs to it.
        let ids = Arc::new(ConnIds::default());
        let (reports, _) = broadcast::channel(REPORTS_BUFFER);
        let (completed, mut completed_rx) = tokio::sync::mpsc::unbounded_channel();
        let reports_for_task = reports.clone();
        let reports_task = AbortOnDropHandle::new(n0_future::task::spawn(async move {
            while let Some(report) = completed_rx.recv().await {
                // No subscriber is a daemon with no send attached, and it is not an error.
                let _ = reports_for_task.send(report);
            }
        }));
        let completion = endpoint::Completion {
            sender: completed,
            ids: Arc::clone(&ids),
        };
        let endpoint = match endpoint::bind_with_completion(
            Some(seed),
            &relay,
            Congestion::Cubic,
            saved,
            completion.clone(),
        )
        .await
        {
            Ok(endpoint) => endpoint,
            // A port a previous send saved can be taken. A daemon that cannot start is worse than
            // one on another port, and its pairing ticket carries whatever port it got.
            Err(_) if saved.is_some() => {
                endpoint::bind_with_completion(
                    Some(seed),
                    &relay,
                    Congestion::Cubic,
                    None,
                    completion,
                )
                .await?
            }
            Err(e) => return Err(e),
        };
        if let Some(port) = endpoint
            .bound_sockets()
            .first()
            .map(std::net::SocketAddr::port)
        {
            home.save_port(port);
        }
        if !matches!(relay, RelayModeChoice::Disabled | RelayModeChoice::Loopback) {
            endpoint::wait_for_online(&endpoint).await?;
        }
        endpoint::wait_for_direct_addr(&endpoint).await?;

        let devices = Devices::open(home.path());
        let pairing = Arc::new(Pairing::new(devices.clone()));
        let held = Held::default();
        let slots = Slots::default();
        let mut router = Router::builder(endpoint.clone())
            .accept(ALPN_BLOBS, AttachedBlobs(slots.clone()))
            .accept(ALPN, AttachedSluice(slots.clone()))
            .accept(
                ALPN_PAIR,
                PairHandler {
                    pairing: Arc::clone(&pairing),
                    limits: Limits::default(),
                    held: held.clone(),
                },
            );
        if let Some(inbox) = inbox {
            std::fs::create_dir_all(&inbox.path)
                .map_err(|e| Error::Storage(format!("cannot create the inbox: {e}")))?;
            let deliver = Arc::new(Deliverer::new(inbox, devices.clone()));
            router = router.accept(
                ALPN_DELIVER,
                DeliverHandler {
                    deliver,
                    devices: devices.clone(),
                    held: held.clone(),
                },
            );
        }
        let listener = Arc::new(Self {
            dir: home.path().to_path_buf(),
            endpoint,
            _router: router.spawn(),
            devices,
            pairing,
            held,
            send_lock: Arc::default(),
            slots,
            ids,
            reports,
            _reports_task: reports_task,
            stopped: AtomicBool::new(false),
        });
        Ok(listener)
    }

    /// The endpoint id a pairing ticket names.
    pub fn id(&self) -> [u8; 32] {
        *self.endpoint.id().as_bytes()
    }

    /// Makes one pairing ticket for `grant`, valid for `ttl` seconds or [`DEFAULT_TTL`] of them.
    /// More than one hour is refused: a pairing ticket is a bearer invitation.
    pub fn pair(&self, grant: Grant, ttl: Option<u64>) -> Result<PairTicket, Error> {
        let ttl = ttl.unwrap_or(DEFAULT_TTL);
        if ttl == 0 || ttl > MAX_TTL {
            return Err(Error::Policy(format!(
                "a pairing ticket lives 1 to {MAX_TTL} seconds"
            )));
        }
        let addr = self.endpoint.addr();
        let addrs: Vec<SocketAddr> = addr
            .addrs
            .iter()
            .filter_map(|addr| match addr {
                iroh::TransportAddr::Ip(ip) => Some(*ip),
                _ => None,
            })
            .collect();
        let relay = addr.addrs.iter().find_map(|addr| match addr {
            iroh::TransportAddr::Relay(url) => Some(url.to_string()),
            _ => None,
        });
        let secret = iroh::SecretKey::generate().to_bytes();
        let expires_ms = now_ms().saturating_add(ttl.saturating_mul(1000));
        let ticket = Ticket::new(
            self.id(),
            addrs,
            relay,
            Some(expires_ms),
            Body::Pairing { secret, grant },
        )?;
        let text = ticket.encode();
        self.pairing.remember(secret, grant, expires_ms);
        Ok(PairTicket { text, expires_ms })
    }

    /// The grants file, for the device calls.
    pub fn devices(&self) -> &Devices {
        &self.devices
    }

    /// Removes a grant, forgets the ticket that made it, and closes that peer's connections.
    pub fn revoke(&self, endpoint_id: &[u8; 32]) -> Result<bool, Error> {
        let removed = self.devices.revoke(endpoint_id)?;
        if removed.is_some() {
            self.pairing.forget(*endpoint_id);
            self.held.close(*endpoint_id);
        }
        Ok(removed.is_some())
    }

    /// Registers this listener for `home`, so a send started for that home attaches to it.
    pub fn register(self: &Arc<Self>) {
        let map = registry();
        let mut map = map.lock().unwrap_or_else(|p| p.into_inner());
        map.insert(self.dir.clone(), Arc::downgrade(self));
    }

    /// Stops taking sends for `home` and shuts the endpoint down.
    pub fn stop(&self) {
        if self.stopped.swap(true, Ordering::SeqCst) {
            return;
        }
        registry()
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&self.dir);
    }

    /// The endpoint, for a caller that has to dial it in a test.
    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

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

/// The listeners of this process, by the home they serve.
fn registry() -> &'static Mutex<HashMap<PathBuf, Weak<Listener>>> {
    static LISTENERS: OnceLock<Mutex<HashMap<PathBuf, Weak<Listener>>>> = OnceLock::new();
    LISTENERS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The listener serving `home`, if this process started one.
fn listener_for(home: &Home) -> Option<Arc<Listener>> {
    let map = registry().lock().unwrap_or_else(|p| p.into_inner());
    map.get(home.path()).and_then(Weak::upgrade)
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
pub(super) async fn attach_for(
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
struct Slots {
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
struct AttachedBlobs(Slots);

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
struct AttachedSluice(Slots);

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
