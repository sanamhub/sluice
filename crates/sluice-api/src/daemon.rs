//! The daemon: the service behind a listener that only this user can reach, until a signal or a
//! client asks it to stop.

use std::future::Future;

use sluice_core::Home;
use sluice_core::engine::RelayModeChoice;

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
    /// served until [`Daemon::serve`].
    pub fn bind(home: Home, roots: Roots, relay: RelayModeChoice) -> Result<Self, ApiError> {
        roots.refuse_home(home.path())?;
        let (incoming, guard, location) = platform::listen(home.path())?;
        Ok(Self {
            service: Service::new(home, roots, relay),
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
        drop(accepting);
        result
    }
}
