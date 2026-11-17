//! The daemon: the service behind a listener that only this user can reach, until a signal or a
//! client asks it to stop.

use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;

use sluice_core::Home;
use sluice_core::engine::{InboxFolder, Listener, RelayModeChoice};

use crate::error::ApiError;
use crate::grpc::{self, Grpc};
use crate::roots::Roots;
use crate::service::{STOP_WAIT, Service};
use crate::transport::{Incoming, platform};

/// A daemon that is listening and has not started serving.
pub struct Daemon {
    service: Service,
    incoming: Incoming<platform::Stream>,
    // Removes the socket file when the daemon ends.
    guard: platform::Guard,
    location: String,
}

impl Daemon {
    /// Makes the listener for the home `home` holds, and builds the service over it. Nothing is
    /// served until [`Daemon::serve`]. The identity endpoint starts when the home has an
    /// `identity.key`, and delivery is served only when `inbox` is given.
    pub async fn bind(
        home: Home,
        roots: Roots,
        relay: RelayModeChoice,
        inbox: Option<PathBuf>,
        inbox_quota: u64,
    ) -> Result<Self, ApiError> {
        roots.refuse_home(home.path())?;
        let (incoming, guard, location) = platform::listen(home.path())?;
        let listener = start_listener(&home, &roots, &relay, inbox, inbox_quota).await?;
        Ok(Self {
            service: Service::new(home, roots, relay, listener),
            incoming,
            guard,
            location,
        })
    }

    /// The socket path or pipe name the daemon listens on.
    pub fn location(&self) -> &str {
        &self.location
    }

    /// The service, for a caller in the same process.
    pub fn service(&self) -> Service {
        self.service.clone()
    }

    /// Serves until `shutdown` resolves or a client calls `shutdown`. Running jobs end as
    /// cancelled with their stores kept, and the listener is removed before this returns.
    pub async fn serve(self, shutdown: impl Future<Output = ()>) -> Result<(), ApiError> {
        let Self {
            service,
            incoming,
            guard: _guard,
            location: _,
        } = self;
        // The accept task runs until this function ends.
        let Incoming {
            connections,
            _task: accepting,
        } = incoming;
        let stopping = {
            let service = service.clone();
            async move {
                tokio::select! {
                    () = shutdown => {}
                    () = service.stop_requested() => {}
                }
                // The jobs are told to stop first, so their watchers see the end and the server can
                // finish its calls.
                service.shutdown();
            }
        };
        let served = grpc::builder()
            .add_service(Grpc::server(service.clone()))
            .serve_with_incoming_shutdown(connections, stopping);
        tokio::pin!(served);
        // A client that holds a connection open must not keep a stopping daemon alive.
        let forced = async {
            service.stop_requested().await;
            tokio::time::sleep(STOP_WAIT).await;
        };
        let result = tokio::select! {
            result = &mut served => result.map_err(|e| ApiError::Failed(format!("the server failed: {e}"))),
            () = forced => Ok(()),
        };
        service.stop_jobs(STOP_WAIT).await;
        // The endpoint stops taking sends before the daemon's process goes, so a send started in
        // that moment binds its own endpoint instead of attaching to one that is closing.
        service.stop_listener();
        drop(accepting);
        result
    }
}

/// Starts the daemon's identity endpoint when the home has an identity key, and serves delivery
/// only when `inbox` is given.
async fn start_listener(
    home: &Home,
    roots: &Roots,
    relay: &RelayModeChoice,
    inbox: Option<PathBuf>,
    inbox_quota: u64,
) -> Result<Option<Arc<Listener>>, ApiError> {
    if !home.path().join("identity.key").is_file() {
        if inbox.is_some() {
            return Err(ApiError::Refused(
                "the home has no identity.key, so the daemon cannot receive deliveries; run \
                 `sluice id` first"
                    .to_string(),
            ));
        }
        return Ok(None);
    }
    let inbox = match inbox {
        Some(path) => {
            std::fs::create_dir_all(&path).map_err(|e| {
                ApiError::Refused(format!("cannot make the inbox {}: {e}", path.display()))
            })?;
            if !roots.contains(&path) {
                return Err(ApiError::Refused(format!(
                    "the inbox {} is not inside a root the daemon was started with",
                    path.display()
                )));
            }
            Some(InboxFolder {
                path,
                quota: inbox_quota,
            })
        }
        None => None,
    };
    Listener::bind(home, relay.clone(), inbox)
        .await
        .map(Some)
        .map_err(|error| ApiError::from_core(&error, ""))
}
