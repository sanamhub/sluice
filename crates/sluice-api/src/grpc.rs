//! The gRPC face of the service: each call of the generated server trait is one method of
//! [`Service`], and the only code here is the conversion of its answer into a status.

use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};

use crate::proto::control_service_server::{ControlService, ControlServiceServer};
use crate::proto::{self, WatchEventsResponse};
use crate::service::Service;

/// The largest message either side sends or accepts. The control plane carries no file byte, and
/// the biggest answer is a list of names, so a megabyte is far more than a call needs.
pub const MAX_MESSAGE_BYTES: usize = 1024 * 1024;

/// How many calls one connection may have open at once.
const MAX_STREAMS: u32 = 64;

/// The service as the generated server wants it.
pub(crate) struct Grpc(Service);

impl Grpc {
    /// The server for `service`, with the message size limit in both directions.
    pub(crate) fn server(service: Service) -> ControlServiceServer<Self> {
        ControlServiceServer::new(Self(service))
            .max_decoding_message_size(MAX_MESSAGE_BYTES)
            .max_encoding_message_size(MAX_MESSAGE_BYTES)
    }
}

/// The transport's server builder with the daemon's limits.
pub(crate) fn builder() -> tonic::transport::Server {
    tonic::transport::Server::builder().max_concurrent_streams(Some(MAX_STREAMS))
}

#[tonic::async_trait]
impl ControlService for Grpc {
    async fn start_send(
        &self,
        request: Request<proto::StartSendRequest>,
    ) -> Result<Response<proto::StartSendResponse>, Status> {
        Ok(Response::new(
            self.0.start_send(request.into_inner()).await?,
        ))
    }

    async fn start_receive(
        &self,
        request: Request<proto::StartReceiveRequest>,
    ) -> Result<Response<proto::StartReceiveResponse>, Status> {
        Ok(Response::new(
            self.0.start_receive(request.into_inner()).await?,
        ))
    }

    async fn inspect_ticket(
        &self,
        request: Request<proto::InspectTicketRequest>,
    ) -> Result<Response<proto::InspectTicketResponse>, Status> {
        Ok(Response::new(
            self.0.inspect_ticket(request.into_inner()).await?,
        ))
    }

    async fn list_jobs(
        &self,
        request: Request<proto::ListJobsRequest>,
    ) -> Result<Response<proto::ListJobsResponse>, Status> {
        Ok(Response::new(self.0.list_jobs(&request.into_inner())))
    }

    type WatchEventsStream = ReceiverStream<Result<WatchEventsResponse, Status>>;

    async fn watch_events(
        &self,
        request: Request<proto::WatchEventsRequest>,
    ) -> Result<Response<Self::WatchEventsStream>, Status> {
        let mut watcher = self.0.watch_events(&request.into_inner())?;
        // One slot: the buffer that matters is the watcher's, which has a bound and a gap marker.
        let (tx, rx) = mpsc::channel(1);
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    () = tx.closed() => break,
                    next = watcher.next() => {
                        let Some(event) = next else { break };
                        let item = WatchEventsResponse { event: Some(event) };
                        if tx.send(Ok(item)).await.is_err() {
                            break;
                        }
                    }
                }
            }
        });
        Ok(Response::new(ReceiverStream::new(rx)))
    }

    async fn cancel_job(
        &self,
        request: Request<proto::CancelJobRequest>,
    ) -> Result<Response<proto::CancelJobResponse>, Status> {
        Ok(Response::new(self.0.cancel_job(&request.into_inner())?))
    }

    async fn version(
        &self,
        _request: Request<proto::VersionRequest>,
    ) -> Result<Response<proto::VersionResponse>, Status> {
        Ok(Response::new(self.0.version()))
    }

    async fn doctor(
        &self,
        _request: Request<proto::DoctorRequest>,
    ) -> Result<Response<proto::DoctorResponse>, Status> {
        Ok(Response::new(self.0.doctor().await?))
    }

    async fn list_roots(
        &self,
        _request: Request<proto::ListRootsRequest>,
    ) -> Result<Response<proto::ListRootsResponse>, Status> {
        Ok(Response::new(self.0.list_roots()))
    }

    async fn list_entries(
        &self,
        request: Request<proto::ListEntriesRequest>,
    ) -> Result<Response<proto::ListEntriesResponse>, Status> {
        Ok(Response::new(
            self.0.list_entries(request.into_inner()).await?,
        ))
    }

    async fn create_pairing(
        &self,
        request: Request<proto::CreatePairingRequest>,
    ) -> Result<Response<proto::CreatePairingResponse>, Status> {
        Ok(Response::new(self.0.create_pairing(request.into_inner())?))
    }

    async fn list_devices(
        &self,
        _request: Request<proto::ListDevicesRequest>,
    ) -> Result<Response<proto::ListDevicesResponse>, Status> {
        Ok(Response::new(self.0.list_devices()?))
    }

    async fn rename_device(
        &self,
        request: Request<proto::RenameDeviceRequest>,
    ) -> Result<Response<proto::RenameDeviceResponse>, Status> {
        Ok(Response::new(self.0.rename_device(request.into_inner())?))
    }

    async fn revoke_device(
        &self,
        request: Request<proto::RevokeDeviceRequest>,
    ) -> Result<Response<proto::RevokeDeviceResponse>, Status> {
        Ok(Response::new(self.0.revoke_device(request.into_inner())?))
    }

    async fn shutdown(
        &self,
        _request: Request<proto::ShutdownRequest>,
    ) -> Result<Response<proto::ShutdownResponse>, Status> {
        Ok(Response::new(self.0.shutdown()))
    }
}
