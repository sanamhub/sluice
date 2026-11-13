//! The jobs of one daemon: what each is, what has happened to it, and how to stop it. A job names
//! no ticket anywhere in this module: the ticket lives in the task that runs the transfer and in
//! nothing else.

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use sluice_core::engine::Event as Engine;
use tokio::sync::watch;

use crate::error::ApiError;
use crate::hub::{Hub, Watcher, event};
use crate::proto::{self, JobKind, JobState, event::Event as Kind};

/// How many jobs run at once.
pub const MAX_RUNNING: usize = 16;

/// How many finished jobs the table keeps for `list_jobs`. The oldest go first.
pub const MAX_FINISHED: usize = 64;

#[derive(Default)]
struct Status {
    bytes_total: u64,
    bytes_done: u64,
    receivers: u32,
    resumable: bool,
    state: Option<JobState>,
    /// The daemon's sentence for a job that failed, which is escaped as a peer's text is.
    failure: Option<String>,
}

/// One job.
pub(crate) struct Job {
    pub(crate) id: String,
    kind: JobKind,
    /// The root of a receive, as the verified event reports it.
    verified_root: Option<String>,
    began: Instant,
    status: Mutex<Status>,
    /// The ticket a send serves, once it is prepared. Only `Service::job_detail` reads it, for a
    /// caller that started the send without waiting for its ticket, and no summary, event or log
    /// line holds it.
    ticket: Mutex<Option<String>>,
    hub: Hub,
    cancel: watch::Sender<bool>,
    ended: watch::Sender<bool>,
}

impl Job {
    fn status(&self) -> MutexGuard<'_, Status> {
        self.status
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub(crate) fn summary(&self) -> proto::JobSummary {
        let status = self.status();
        proto::JobSummary {
            id: self.id.clone(),
            kind: self.kind as i32,
            state: status.state.unwrap_or(JobState::Running) as i32,
            bytes_total: status.bytes_total,
            bytes_done: status.bytes_done,
            receivers: status.receivers,
            resumable: status.resumable,
        }
    }

    /// Keeps the ticket of a send for the one call that may read it back.
    pub(crate) fn keep_ticket(&self, ticket: String) {
        *self
            .ticket
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(ticket);
    }

    /// The summary, the ticket if one was kept, and the failure sentence if the job failed.
    pub(crate) fn detail(&self) -> (proto::JobSummary, Option<String>, Option<String>) {
        let ticket = self
            .ticket
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        (self.summary(), ticket, self.status().failure.clone())
    }

    pub(crate) fn is_running(&self) -> bool {
        self.status().state.is_none()
    }

    pub(crate) fn watch(&self) -> Result<Watcher, ApiError> {
        self.hub.watch(&self.id)
    }

    /// Asks the task to stop. Returns whether the job was still running.
    pub(crate) fn cancel(&self) -> bool {
        let running = self.is_running();
        self.cancel.send_replace(true);
        running
    }

    /// Resolves when `cancel` is called.
    pub(crate) async fn cancelled(&self) {
        let mut asked = self.cancel.subscribe();
        // The sender lives as long as the job, so an error here cannot happen; waiting for ever is
        // the right answer to it anyway.
        if asked.wait_for(|asked| *asked).await.is_err() {
            std::future::pending::<()>().await;
        }
    }

    /// Resolves when the job has ended.
    pub(crate) async fn ended(&self) {
        let mut ended = self.ended.subscribe();
        let _ = ended.wait_for(|ended| *ended).await;
    }

    /// Records and publishes one thing the engine reported.
    pub(crate) fn observe(&self, engine: &Engine) {
        if let Some(kind) = self.translate(engine) {
            self.publish(kind);
        }
    }

    fn publish(&self, kind: Kind) {
        self.hub.publish(&event(&self.id, kind));
    }

    /// Ends the job: the state, the one sentence for a failure, and the last event.
    pub(crate) fn finish(&self, state: JobState, message: String, resumable: bool) {
        {
            let mut status = self.status();
            status.state = Some(state);
            status.resumable = resumable;
            if state == JobState::Failed {
                status.failure = Some(message.clone());
            }
        }
        // A ticket is read only while the send runs: once the job has ended nobody can take what it
        // served, and nothing should keep the ticket.
        *self
            .ticket
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        let last = event(
            &self.id,
            Kind::Done(proto::Done {
                state: state as i32,
                message,
                peer_close_reason: None,
            }),
        );
        tracing::info!(job = %self.id, state = state.as_str_name(), "job ended");
        self.hub.finish(&last);
        self.ended.send_replace(true);
    }

    fn translate(&self, engine: &Engine) -> Option<Kind> {
        match engine {
            Engine::Started { files, bytes, .. } | Engine::Planned { files, bytes, .. } => {
                self.status().bytes_total = *bytes;
                Some(Kind::Started(proto::Started {
                    kind: self.kind as i32,
                    files: *files,
                    bytes: *bytes,
                }))
            }
            Engine::Progress { bytes, total } => Some(self.progress(*bytes, *total)),
            Engine::Verified { bytes } => Some(Kind::Verified(proto::Verified {
                root: self.verified_root.clone().unwrap_or_default(),
                bytes: *bytes,
            })),
            Engine::ReceiverFinished { whole: true, .. } => {
                self.status().receivers += 1;
                None
            }
            Engine::ReceiverRefused { places, .. } => Some(warning(format!(
                "a receiver was refused because all {places} places of this send are taken"
            ))),
            other => path_event(other),
        }
    }

    fn progress(&self, done: u64, total: u64) -> Kind {
        let seconds = self.began.elapsed().as_secs_f64().max(0.001);
        let mut status = self.status();
        status.bytes_done = done;
        status.bytes_total = total;
        Kind::Progress(proto::Progress {
            bytes_done: done,
            bytes_total: total,
            bytes_per_second: (done as f64 / seconds) as u64,
        })
    }
}

fn warning(message: String) -> Kind {
    Kind::Warning(proto::Warning {
        message,
        name: None,
        relay: None,
    })
}

fn relay(text: &Option<String>) -> Option<proto::PeerRelayUrl> {
    text.as_ref()
        .map(|text| proto::PeerRelayUrl { text: text.clone() })
}

fn path_kind(kind: sluice_core::engine::PathKind) -> i32 {
    match kind {
        sluice_core::engine::PathKind::Direct => proto::PathKind::Direct as i32,
        sluice_core::engine::PathKind::Relayed => proto::PathKind::Relayed as i32,
    }
}

/// The events about the connection and the retries, which carry no job state.
fn path_event(engine: &Engine) -> Option<Kind> {
    match engine {
        Engine::Connected { path, relay: url } => Some(Kind::Connected(proto::Connected {
            path: path_kind(*path),
            relay: relay(url),
        })),
        Engine::PathChanged { path, relay: url } => Some(Kind::PathChanged(proto::PathChanged {
            path: path_kind(*path),
            relay: relay(url),
        })),
        Engine::Relayed {
            relay: url,
            seconds,
        } => Some(Kind::Relayed(proto::Relayed {
            relay: Some(proto::PeerRelayUrl { text: url.clone() }),
            waited_seconds: *seconds,
        })),
        Engine::Retrying { attempt, seconds } => Some(Kind::Retrying(proto::Retrying {
            attempt: *attempt,
            reason: format!("the connection was lost, so the next try is in {seconds} seconds"),
        })),
        Engine::Warning { message } => Some(warning(sluice_core::manifest::path::shown(message))),
        _ => None,
    }
}

/// A job id: 128 random bits as 32 hex characters, so an id says nothing about how many jobs came
/// before it and cannot be guessed by another program of the same user.
fn random_id() -> Result<String, ApiError> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes)
        .map_err(|_| ApiError::Failed("the system has no random bytes to give".to_string()))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// The table of jobs, newest last.
#[derive(Default)]
pub(crate) struct Table {
    jobs: Mutex<Vec<Arc<Job>>>,
}

impl Table {
    fn jobs(&self) -> MutexGuard<'_, Vec<Arc<Job>>> {
        self.jobs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// A new running job, or the refusal that too many already run.
    pub(crate) fn start(
        &self,
        kind: JobKind,
        verified_root: Option<String>,
    ) -> Result<Arc<Job>, ApiError> {
        let mut jobs = self.jobs();
        if jobs.iter().filter(|job| job.is_running()).count() >= MAX_RUNNING {
            return Err(ApiError::Busy(format!(
                "this daemon runs at most {MAX_RUNNING} jobs at once"
            )));
        }
        let finished = jobs.iter().filter(|job| !job.is_running()).count();
        for _ in MAX_FINISHED..=finished {
            if let Some(oldest) = jobs.iter().position(|job| !job.is_running()) {
                jobs.remove(oldest);
            }
        }
        let job = Arc::new(Job {
            id: random_id()?,
            kind,
            verified_root,
            began: Instant::now(),
            status: Mutex::new(Status::default()),
            ticket: Mutex::new(None),
            hub: Hub::default(),
            cancel: watch::channel(false).0,
            ended: watch::channel(false).0,
        });
        jobs.push(job.clone());
        Ok(job)
    }

    pub(crate) fn get(&self, id: &str) -> Result<Arc<Job>, ApiError> {
        self.jobs()
            .iter()
            .find(|job| job.id == id)
            .cloned()
            .ok_or_else(|| ApiError::NotFound("this daemon has no job with that id".to_string()))
    }

    pub(crate) fn list(&self, only: JobState) -> Vec<proto::JobSummary> {
        self.jobs()
            .iter()
            .map(|job| job.summary())
            .filter(|summary| only == JobState::Unspecified || summary.state == only as i32)
            .collect()
    }

    /// Asks every running job to stop, and returns those that were running.
    pub(crate) fn cancel_all(&self) -> Vec<Arc<Job>> {
        let running: Vec<Arc<Job>> = self
            .jobs()
            .iter()
            .filter(|job| job.is_running())
            .cloned()
            .collect();
        for job in &running {
            job.cancel();
        }
        running
    }
}

#[cfg(test)]
mod tests;
