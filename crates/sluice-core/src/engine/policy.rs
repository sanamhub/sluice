//! The one access policy of a provider. It answers `get`, `get_many` and `observe` with the same
//! set of allowed hashes, refuses every push, and refuses everything after the ticket expiry.

use std::{
    collections::BTreeSet,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use iroh_blobs::{
    Hash,
    provider::events::{
        AbortReason, ConnectMode, EventMask, EventSender, ObserveMode, ProviderMessage,
        RequestMode, RequestUpdate,
    },
};
use n0_future::task::AbortOnDropHandle;
use tokio::sync::mpsc::UnboundedSender;

/// The provider's policy and the task that answers its request events.
pub struct Policy {
    /// Handed to `BlobsProtocol`, which uses it for every request.
    pub events: EventSender,
    /// Aborted when the provider goes away, so no event task outlives it.
    pub _task: AbortOnDropHandle<()>,
}

impl std::fmt::Debug for Policy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Policy").finish_non_exhaustive()
    }
}

/// What the provider reports back so a sender can stop after one download.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServeEvent {
    /// A request for this hash completed, on this connection.
    Downloaded {
        /// The blob the receiver asked for.
        hash: Hash,
        /// The connection the request arrived on.
        connection: u64,
    },
    /// That connection closed, so the peer has everything that was written to it.
    Closed(u64),
}

/// Starts the policy. `done` receives every completed get and every closed connection, so a sender
/// can stop after one download and not cut the last bytes off. `expiry` is UTC milliseconds; at or
/// after it every request is refused.
pub fn start(
    allowed: BTreeSet<Hash>,
    expiry: Option<u64>,
    done: UnboundedSender<ServeEvent>,
) -> Policy {
    let mask = EventMask {
        connected: ConnectMode::Notify,
        get: RequestMode::InterceptLog,
        get_many: RequestMode::Intercept,
        push: RequestMode::Disabled,
        observe: ObserveMode::Intercept,
        throttle: Default::default(),
    };
    let (events, mut rx) = EventSender::channel(32, mask);
    let allowed = Arc::new(allowed);
    let task = AbortOnDropHandle::new(n0_future::task::spawn(async move {
        while let Some(message) = rx.recv().await {
            match message {
                ProviderMessage::GetRequestReceived(mut msg) => {
                    let hash = msg.request.hash;
                    // Any range of an allowed hash is served, including a hash sequence request
                    // for the manifest root. The allowed set is the access rule, not the format.
                    let ok = !expired(expiry) && allowed.contains(&hash);
                    let answer = if ok {
                        Ok(())
                    } else {
                        Err(AbortReason::Permission)
                    };
                    let connection = msg.connection_id;
                    if msg.tx.send(answer).await.is_err() || !ok {
                        continue;
                    }
                    let done = done.clone();
                    n0_future::task::spawn(async move {
                        while let Ok(Some(update)) = msg.rx.recv().await {
                            if matches!(update, RequestUpdate::Completed(_)) {
                                let _ = done.send(ServeEvent::Downloaded { hash, connection });
                                break;
                            }
                            if matches!(update, RequestUpdate::Aborted(_)) {
                                break;
                            }
                        }
                    });
                }
                ProviderMessage::GetManyRequestReceived(msg) => {
                    let ok = !expired(expiry)
                        && msg.request.hashes.iter().all(|hash| allowed.contains(hash));
                    let answer = if ok {
                        Ok(())
                    } else {
                        Err(AbortReason::Permission)
                    };
                    msg.tx.send(answer).await.ok();
                }
                ProviderMessage::ObserveRequestReceived(msg) => {
                    let ok = !expired(expiry) && allowed.contains(&msg.request.hash);
                    let answer = if ok {
                        Ok(())
                    } else {
                        Err(AbortReason::Permission)
                    };
                    msg.tx.send(answer).await.ok();
                }
                ProviderMessage::ConnectionClosed(msg) => {
                    let _ = done.send(ServeEvent::Closed(msg.connection_id));
                }
                // Push is disabled in the mask, so a push is refused before this task sees it.
                _ => {}
            }
        }
    }));
    Policy {
        events,
        _task: task,
    }
}

fn expired(expiry: Option<u64>) -> bool {
    let Some(expiry) = expiry else {
        return false;
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64);
    now >= expiry
}

#[cfg(test)]
mod tests {
    use super::expired;

    #[test]
    fn the_boundary_is_the_ticket_boundary() {
        assert!(!expired(None));
        assert!(expired(Some(0)));
        assert!(!expired(Some(u64::MAX)));
    }
}
