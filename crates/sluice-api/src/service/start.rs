//! Starting a send and a receive: the checks come first, then the transfer runs as a task of its own
//! that the job's cancel stops. The ticket is read here and handed to that task and to nothing else.

use std::path::PathBuf;
use std::sync::Arc;

use sluice_core::Error;
use sluice_core::engine::{
    Congestion, Keep, ObjectId, Outgoing, Pins, ReceiptTarget, ReceiveOptions, RelayModeChoice,
    StopEnd, deadline, now_ms, prepare_send_paths_confined, receive_checked,
};
use sluice_core::ticket::{Body, Ticket};
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};
use tokio::sync::oneshot;

use super::{Service, decode_refusal, ticket_text};
use crate::error::{ApiError, message_of};
use crate::jobs::Job;
use crate::proto::{self, JobKind, JobState};
use crate::roots::{Opened, Use};

/// How many paths one send may name.
const MAX_SEND_PATHS: usize = 1024;

impl Service {
    /// Starts a send of paths inside a root and returns the ticket it serves. The send is prepared
    /// by a task of its own, so a caller that goes away while it is prepared leaves a job that
    /// ends, and not one that serves a ticket nobody has.
    pub async fn start_send(
        &self,
        request: proto::StartSendRequest,
    ) -> Result<proto::StartSendResponse, ApiError> {
        self.refuse_new_work()?;
        self.refuse_without_roots()?;
        let opened = self.send_paths(&request)?;
        let keep = keep_of(&request)?;
        let expires = (request.expires_seconds > 0).then_some(request.expires_seconds);
        let job = self.inner.jobs.start(JobKind::Send, None)?;
        let (answer, answered) = oneshot::channel();
        tokio::spawn(send_task(
            self.clone(),
            job,
            opened,
            Plan { keep, expires },
            answer,
        ));
        answered
            .await
            .unwrap_or_else(|_| Err(ApiError::Failed("the send did not start".to_string())))
    }

    /// Starts a receive of a ticket into a folder inside a root.
    pub async fn start_receive(
        &self,
        request: proto::StartReceiveRequest,
    ) -> Result<proto::StartReceiveResponse, ApiError> {
        self.refuse_new_work()?;
        self.refuse_without_roots()?;
        let text = ticket_text(request.ticket)?;
        let ticket = Ticket::decode(&text).map_err(|e| decode_refusal(&e, &text))?;
        let Body::Static { root, .. } = ticket.body() else {
            return Err(ApiError::Refused(
                "the control interface receives fixed transfers; a live ticket is received with \
                 `sluice recv`"
                    .to_string(),
            ));
        };
        let out = self.inner.roots.resolve(request.out.as_ref(), Use::Dir)?;
        let receipt = match request.receipt.as_ref() {
            Some(at) => Some(self.inner.roots.resolve(Some(at), Use::FileName)?),
            None => None,
        };
        let pins = Pins {
            out: Some(out.pin),
            receipt_parent: receipt.as_ref().map(|r| r.pin),
            loopback: matches!(self.inner.relay, RelayModeChoice::Loopback),
        };
        let options = options(
            out.path.clone(),
            request.force,
            receipt.as_ref().map(|r| r.path.clone()),
        );
        let held: Vec<Opened> = std::iter::once(out).chain(receipt).collect();
        let job = self.inner.jobs.start(JobKind::Receive, Some(hex(root)))?;
        let response = proto::StartReceiveResponse {
            job_id: job.id.clone(),
        };
        tracing::info!(job = %job.id, "receive started");
        tokio::spawn(run_receive(job, text, options, pins, held));
        Ok(response)
    }

    /// A daemon that has begun to stop starts nothing.
    fn refuse_new_work(&self) -> Result<(), ApiError> {
        if *self.inner.stop.borrow() {
            return Err(ApiError::Unavailable(
                "this daemon is stopping and starts no new job".to_string(),
            ));
        }
        Ok(())
    }

    /// A daemon with no root refuses before it looks at anything the request carries.
    fn refuse_without_roots(&self) -> Result<(), ApiError> {
        if self.inner.roots.is_empty() {
            return Err(ApiError::Refused(crate::roots::NO_ROOTS.to_string()));
        }
        Ok(())
    }

    /// The paths of a send, each resolved inside its root and each a file that has one name or a
    /// folder.
    fn send_paths(&self, request: &proto::StartSendRequest) -> Result<Vec<Opened>, ApiError> {
        if request.paths.is_empty() {
            return Err(ApiError::Invalid(
                "a send needs at least one path".to_string(),
            ));
        }
        if request.paths.len() > MAX_SEND_PATHS {
            return Err(ApiError::Invalid(format!(
                "a send names at most {MAX_SEND_PATHS} paths"
            )));
        }
        request
            .paths
            .iter()
            .map(|at| {
                let opened = self.inner.roots.resolve(Some(at), Use::Read)?;
                opened.refuse_hard_links()?;
                Ok(opened)
            })
            .collect()
    }

    /// A send of paths inside a root. Every file is copied from a handle proved to be the object the
    /// confined walk recorded, and a single file has the same root as a one-shot send of it has.
    async fn prepare(&self, paths: &[PathBuf], expiry: Option<u64>) -> Result<Outgoing, Error> {
        let relay = self.inner.relay.clone();
        prepare_send_paths_confined(
            &self.inner.home,
            paths,
            relay,
            Congestion::Cubic,
            expiry,
            None,
            None,
        )
        .await
    }
}

/// Refuses when a path no longer names the object the walk opened for it.
fn still_the_same(opened: &[Opened]) -> Result<(), ApiError> {
    let swapped = opened
        .iter()
        .any(|o| ObjectId::of_path(&o.path).ok() != Some(o.pin));
    if swapped {
        return Err(ApiError::Refused(
            "a path was replaced after it was checked, so nothing is sent".to_string(),
        ));
    }
    Ok(())
}

fn keep_of(request: &proto::StartSendRequest) -> Result<Keep, ApiError> {
    match (request.keep, request.keep_for) {
        (true, 0) => Ok(Keep::UntilStopped),
        (false, 0) => Ok(Keep::One),
        (false, count) => Ok(Keep::Receivers(count)),
        (true, _) => Err(ApiError::Invalid(
            "give `keep` or `keep_for`, not both".to_string(),
        )),
    }
}

fn options(out: PathBuf, force: bool, receipt: Option<PathBuf>) -> ReceiveOptions {
    ReceiveOptions {
        out,
        force,
        no_times: false,
        links: false,
        allow_hidden: false,
        limits: super::Service::limits(),
        free_bytes: None,
        max_size: None,
        congestion: Congestion::Cubic,
        rate: None,
        write_limit: None,
        max_network_failures: sluice_core::queue::MAX_NETWORK_FAILURES,
        include: None,
        receipt: receipt.map(|path| ReceiptTarget { path }),
        delta_from: None,
        delta_wait: None,
    }
}

fn hex(bytes: &[u8; 32]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Hands every event the engine reports to the job, until the engine drops its sender.
async fn pump(job: Arc<Job>, mut events: UnboundedReceiver<sluice_core::engine::Event>) {
    while let Some(event) = events.recv().await {
        job.observe(&event);
    }
}

/// How a send is to run.
struct Plan {
    keep: Keep,
    expires: Option<u64>,
}

type Answer = oneshot::Sender<Result<proto::StartSendResponse, ApiError>>;

/// Prepares the send, tells the caller, and serves it. Every way out ends the job.
async fn send_task(
    service: Service,
    job: Arc<Job>,
    opened: Vec<Opened>,
    plan: Plan,
    answer: Answer,
) {
    let paths: Vec<PathBuf> = opened.iter().map(|o| o.path.clone()).collect();
    let prepared = tokio::select! {
        prepared = service.prepare(&paths, deadline(now_ms(), plan.expires)) => prepared,
        () = job.cancelled() => {
            job.finish(JobState::Cancelled, "the job was cancelled".to_string(), false);
            let _ = answer.send(Err(ApiError::Unavailable("the send was cancelled".to_string())));
            return;
        }
    };
    let mut outgoing = match prepared {
        Ok(outgoing) => outgoing,
        Err(error) => {
            job.finish(JobState::Failed, message_of(&error, ""), false);
            let _ = answer.send(Err(ApiError::from_core(&error, "")));
            return;
        }
    };
    // The engine opened the paths itself, so each is checked once more against the object the walk
    // opened. One that was swapped in between is refused and nothing is served.
    if let Err(refused) = still_the_same(&opened) {
        job.finish(JobState::Failed, refused.to_string(), false);
        let _ = answer.send(Err(refused));
        return;
    }
    outgoing.reserve(plan.keep);
    let ticket = outgoing.ticket.encode();
    let response = proto::StartSendResponse {
        ticket: Some(proto::Ticket {
            text: ticket.clone(),
        }),
        job_id: job.id.clone(),
        bytes: outgoing.bytes,
        files: outgoing.files,
    };
    tracing::info!(job = %job.id, files = response.files, bytes = response.bytes, "send started");
    if !hand_over(&job, answer, Ok(response)) {
        return;
    }
    run_send(job, outgoing, plan.keep, opened, ticket).await;
}

/// Gives the caller its answer. A caller that went away has no ticket, so the job it started is
/// ended and not served.
pub(super) fn hand_over<T>(job: &Job, answer: oneshot::Sender<T>, value: T) -> bool {
    if answer.send(value).is_ok() {
        return true;
    }
    job.finish(
        JobState::Cancelled,
        "the call ended before it was answered".to_string(),
        false,
    );
    false
}

async fn run_send(
    job: Arc<Job>,
    outgoing: Outgoing,
    keep: Keep,
    _opened: Vec<Opened>,
    ticket: String,
) {
    let (events, received) = unbounded_channel();
    let pumping = tokio::spawn(pump(job.clone(), received));
    let end = tokio::select! {
        end = outgoing.serve(keep, &events) => Some(end),
        () = job.cancelled() => None,
    };
    drop(events);
    let _ = pumping.await;
    match end {
        None => job.finish(
            JobState::Cancelled,
            "the job was cancelled".to_string(),
            false,
        ),
        Some(Ok(StopEnd::Expired)) => {
            job.finish(JobState::Done, "the ticket expired".to_string(), false);
        }
        Some(Ok(StopEnd::Downloaded)) => job.finish(JobState::Done, String::new(), false),
        Some(Ok(StopEnd::Interrupted)) => {
            job.finish(
                JobState::Cancelled,
                "the send was interrupted".to_string(),
                false,
            );
        }
        Some(Err(error)) => job.finish(JobState::Failed, message_of(&error, &ticket), false),
    }
}

async fn run_receive(
    job: Arc<Job>,
    ticket: String,
    options: ReceiveOptions,
    pins: Pins,
    _opened: Vec<Opened>,
) {
    let (events, received) = unbounded_channel();
    let pumping = tokio::spawn(pump(job.clone(), received));
    let result = tokio::select! {
        result = receive_checked(&ticket, &options, pins, &events) => Some(result),
        () = job.cancelled() => None,
    };
    drop(events);
    let _ = pumping.await;
    match result {
        // The partial store stays beside the output, so the next receive of the ticket resumes.
        None => job.finish(
            JobState::Cancelled,
            "the job was cancelled".to_string(),
            true,
        ),
        Some(Ok(_)) => job.finish(JobState::Done, String::new(), false),
        Some(Err(error)) => {
            let resumable = matches!(error, Error::Network(_) | Error::Cancelled);
            let state = if matches!(error, Error::Cancelled) {
                JobState::Cancelled
            } else {
                JobState::Failed
            };
            job.finish(state, message_of(&error, &ticket), resumable);
        }
    }
}
