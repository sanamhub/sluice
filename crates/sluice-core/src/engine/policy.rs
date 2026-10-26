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

/// Starts the policy. `done` receives the hash of every completed get, so a sender can stop after
/// one download. `expiry` is UTC milliseconds; at or after it every request is refused.
pub fn start(allowed: BTreeSet<Hash>, expiry: Option<u64>, done: UnboundedSender<Hash>) -> Policy {
    let mask = EventMask {
        connected: ConnectMode::None,
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
                    let ok =
                        !expired(expiry) && msg.request.ranges.is_blob() && allowed.contains(&hash);
                    let answer = if ok {
                        Ok(())
                    } else {
                        Err(AbortReason::Permission)
                    };
                    if msg.tx.send(answer).await.is_err() || !ok {
                        continue;
                    }
                    let done = done.clone();
                    n0_future::task::spawn(async move {
                        while let Ok(Some(update)) = msg.rx.recv().await {
                            if matches!(update, RequestUpdate::Completed(_)) {
                                let _ = done.send(hash);
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
