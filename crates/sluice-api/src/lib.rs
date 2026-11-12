//! The local control interface of Sluice: the generated types of `proto/sluice/v1`, the service
//! that answers them over the engine, the job table, and the daemon that serves them on a socket
//! or pipe only the current user can reach.
//!
//! [`Service`] is one plain struct with an async method per call, so a caller in the same process
//! needs no socket: the gRPC adapter and the MCP server both call it directly. No file byte ever
//! passes through this interface.

mod client;
mod daemon;
mod error;
mod grpc;
mod hub;
mod jobs;
mod roots;
mod service;
mod transport;

/// The messages and the gRPC service of `proto/sluice/v1/control.proto`, generated and checked in so
/// that building the workspace needs no `protoc`. `tests/generated.rs` fails when this copy is not
/// what the schema produces.
#[allow(missing_docs, clippy::all)]
#[rustfmt::skip]
#[path = "gen/sluice.v1.rs"]
pub mod proto;

pub use client::{Client, connect};
pub use daemon::Daemon;
pub use error::ApiError;
pub use grpc::MAX_MESSAGE_BYTES;
pub use hub::{MAX_WATCHERS, WATCH_BUFFER, Watcher};
pub use jobs::{MAX_FINISHED, MAX_RUNNING};
pub use roots::{NO_ROOTS, Roots};
pub use service::{JobDetail, STOP_WAIT, Service};
pub use transport::MAX_CONNECTIONS;
