//! A client of the daemon's control interface, over the same socket or pipe the daemon listens on.
//! `sluice daemon status` and `sluice daemon stop` are thin callers of it.

use std::io;
use std::path::Path;
use std::time::Duration;

use hyper_util::rt::TokioIo;
use tonic::transport::{Channel, Endpoint, Uri};
use tower::service_fn;

use crate::error::ApiError;
use crate::grpc::MAX_MESSAGE_BYTES;
use crate::proto::control_service_client::ControlServiceClient;
use crate::transport::platform;

/// How long a client waits to connect.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// The generated client over a channel to a daemon.
pub type Client = ControlServiceClient<Channel>;

/// Connects to the daemon of the home at `home`. The refusal says where it looked and how to start
/// one.
pub async fn connect(home: &Path) -> Result<Client, ApiError> {
    let target = platform::target(home);
    let place = platform::describe(&target);
    // The address is never dialled: the connector below opens the socket or the pipe.
    let endpoint = Endpoint::from_static("http://control.invalid").connect_timeout(CONNECT_TIMEOUT);
    let channel = endpoint
        .connect_with_connector(service_fn(move |_: Uri| {
            let target = target.clone();
            async move { Ok::<_, io::Error>(TokioIo::new(platform::open(&target).await?)) }
        }))
        .await
        .map_err(|_| {
            ApiError::Unavailable(format!(
                "no daemon answers at {place}; start one with `sluice daemon`"
            ))
        })?;
    Ok(ControlServiceClient::new(channel)
        .max_decoding_message_size(MAX_MESSAGE_BYTES)
        .max_encoding_message_size(MAX_MESSAGE_BYTES))
}
