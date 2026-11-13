//! The service: one plain struct with an async method for each call of the control interface. The
//! gRPC adapter, and later the MCP server, call these methods in process, so a caller needs no
//! socket and no listener of our own to use them.

use std::sync::Arc;
use std::time::Duration;

use sluice_core::Home;
use sluice_core::engine::RelayModeChoice;
use sluice_core::manifest::Limits;
use tokio::sync::{Semaphore, watch};

use crate::error::{ApiError, message_of};
use crate::hub::Watcher;
use crate::jobs::Table;
use crate::proto::{self, JobState};
use crate::roots::Roots;

mod doctor;
mod inspect;
mod run;
mod start;

/// How many tickets are inspected at once.
const MAX_INSPECTS: usize = 4;

/// How long a ticket's manifest may take to read.
const INSPECT_TIMEOUT: Duration = Duration::from_secs(60);

/// How long a stopping daemon waits for its jobs to end.
pub const STOP_WAIT: Duration = Duration::from_secs(10);

/// The version of the control interface this daemon serves.
const PROTOCOL: u32 = 1;

/// A job with what only its starter may read.
#[derive(Debug)]
pub struct JobDetail {
    /// The job as `list_jobs` shows it.
    pub summary: proto::JobSummary,
    /// The ticket a send serves, once prepared. A secret.
    pub secret_ticket: Option<String>,
    /// The sentence for a job that failed, escaped as a peer's text is.
    pub failure: Option<String>,
}

/// One Sluice engine in one home. Cheap to clone: the clones share the jobs and the roots.
#[derive(Clone)]
pub struct Service {
    inner: Arc<Inner>,
}

struct Inner {
    home: Home,
    roots: Roots,
    relay: RelayModeChoice,
    jobs: Table,
    inspects: Semaphore,
    stop: watch::Sender<bool>,
}

impl Service {
    /// A service over `home`, with the folders it may use and the relay its transfers use.
    pub fn new(home: Home, roots: Roots, relay: RelayModeChoice) -> Self {
        Self {
            inner: Arc::new(Inner {
                home,
                roots,
                relay,
                jobs: Table::default(),
                inspects: Semaphore::new(MAX_INSPECTS),
                stop: watch::channel(false).0,
            }),
        }
    }

    /// The jobs the daemon knows about, newest last.
    pub fn list_jobs(&self, request: &proto::ListJobsRequest) -> proto::ListJobsResponse {
        let only = JobState::try_from(request.state).unwrap_or(JobState::Unspecified);
        proto::ListJobsResponse {
            jobs: self.inner.jobs.list(only),
        }
    }

    /// One job, and what only a caller that started it may read: the ticket of a send once it is
    /// prepared, and the sentence for a job that failed. The gRPC interface never gives the ticket
    /// out; a caller in this process that started a send without waiting for it does.
    pub fn job_detail(&self, job_id: &str) -> Result<JobDetail, ApiError> {
        let (summary, secret_ticket, failure) = self.inner.jobs.get(job_id)?.detail();
        Ok(JobDetail {
            summary,
            secret_ticket,
            failure,
        })
    }

    /// One job's events from now on.
    pub fn watch_events(&self, request: &proto::WatchEventsRequest) -> Result<Watcher, ApiError> {
        self.inner.jobs.get(&request.job_id)?.watch()
    }

    /// Stops a job.
    pub fn cancel_job(
        &self,
        request: &proto::CancelJobRequest,
    ) -> Result<proto::CancelJobResponse, ApiError> {
        let job = self.inner.jobs.get(&request.job_id)?;
        Ok(proto::CancelJobResponse {
            was_running: job.cancel(),
        })
    }

    /// The version of this daemon and of the protocol.
    pub fn version(&self) -> proto::VersionResponse {
        proto::VersionResponse {
            version: env!("CARGO_PKG_VERSION").to_string(),
            protocol: PROTOCOL,
        }
    }

    /// Asks the daemon to stop. The jobs that run end as cancelled, and the answer says how many.
    pub fn shutdown(&self) -> proto::ShutdownResponse {
        let running = self.inner.jobs.cancel_all().len();
        self.inner.stop.send_replace(true);
        proto::ShutdownResponse {
            cancelled_jobs: u32::try_from(running).unwrap_or(u32::MAX),
        }
    }

    /// Resolves once a client has asked the daemon to stop.
    pub async fn stop_requested(&self) {
        let mut stop = self.inner.stop.subscribe();
        let _ = stop.wait_for(|stop| *stop).await;
    }

    /// Cancels every job and waits, up to `limit`, until each has ended and closed its store.
    /// Returns how many were running.
    pub async fn stop_jobs(&self, limit: Duration) -> usize {
        let running = self.inner.jobs.cancel_all();
        let wait = async {
            for job in &running {
                job.ended().await;
            }
        };
        let _ = tokio::time::timeout(limit, wait).await;
        running.len()
    }

    fn limits() -> Limits {
        Limits::default()
    }
}

/// The text of a ticket a request carries, which is never copied anywhere but the task that uses it.
fn ticket_text(ticket: Option<proto::Ticket>) -> Result<String, ApiError> {
    match ticket {
        Some(ticket) if !ticket.text.trim().is_empty() => Ok(ticket.text.trim().to_string()),
        _ => Err(ApiError::Invalid(
            "the request carries no ticket".to_string(),
        )),
    }
}

/// A decode error as a sentence that cannot hold the ticket.
fn decode_refusal(error: &sluice_core::Error, ticket: &str) -> ApiError {
    ApiError::Invalid(message_of(error, ticket))
}

#[cfg(test)]
mod tests;
