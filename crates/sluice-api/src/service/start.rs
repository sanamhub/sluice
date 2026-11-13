//! Starting a send and a receive: the checks come first, then the transfer runs as a task of its own
//! that the job's cancel stops. The ticket is read here and handed to that task and to nothing else.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use sluice_core::Error;
use sluice_core::engine::{
    Congestion, Keep, Outgoing, Pins, ReceiptTarget, ReceiveOptions, RelayModeChoice, deadline,
    now_ms, prepare_send_paths_confined,
};
use sluice_core::ticket::{Body, Ticket};
use tokio::sync::oneshot;

use super::run::{run_receive, run_send};
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
            Delivery::Caller(answer),
        ));
        answered
            .await
            .unwrap_or_else(|_| Err(ApiError::Failed("the send did not start".to_string())))
    }

    /// Starts a send and returns its job id at once, before it is prepared. The ticket is read with
    /// [`Service::job_detail`] once the send is prepared; the call that makes a ticket is the only
    /// one that gets it, and here that is the caller of that method.
    pub async fn begin_send(&self, request: proto::StartSendRequest) -> Result<String, ApiError> {
        self.refuse_new_work()?;
        self.refuse_without_roots()?;
        let opened = self.send_paths(&request)?;
        let keep = keep_of(&request)?;
        let expires = (request.expires_seconds > 0).then_some(request.expires_seconds);
        let job = self.inner.jobs.start(JobKind::Send, None)?;
        let id = job.id.clone();
        tokio::spawn(send_task(
            self.clone(),
            job,
            opened,
            Plan { keep, expires },
            Delivery::Job,
        ));
        Ok(id)
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
    pub(super) fn refuse_without_roots(&self) -> Result<(), ApiError> {
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
    async fn prepare(
        &self,
        args: Vec<sluice_core::manifest::ConfinedArg>,
        expiry: Option<u64>,
        cancel: Arc<AtomicBool>,
    ) -> Result<Outgoing, Error> {
        let relay = self.inner.relay.clone();
        prepare_send_paths_confined(
            &self.inner.home,
            args,
            relay,
            Congestion::Cubic,
            expiry,
            None,
            None,
            cancel,
        )
        .await
    }
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

/// How a send is to run.
struct Plan {
    keep: Keep,
    expires: Option<u64>,
}

type Answer = oneshot::Sender<Result<proto::StartSendResponse, ApiError>>;

/// Who is told how the preparing of a send went.
enum Delivery {
    /// A call that waits for the ticket.
    Caller(Answer),
    /// Nobody waits: the ticket is kept with the job.
    Job,
}

impl Delivery {
    /// Gives the outcome over. False when the caller has gone, and the job has been ended.
    fn give(self, job: &Job, outcome: Result<proto::StartSendResponse, ApiError>) -> bool {
        match self {
            Self::Caller(answer) => hand_over(job, answer, outcome),
            Self::Job => {
                if let Ok(response) = outcome
                    && let Some(ticket) = response.ticket
                {
                    job.keep_ticket(ticket.text);
                }
                true
            }
        }
    }
}

/// Prepares the send, tells the caller, and serves it. Every way out ends the job.
async fn send_task(
    service: Service,
    job: Arc<Job>,
    opened: Vec<Opened>,
    plan: Plan,
    delivery: Delivery,
) {
    let args: Result<Vec<_>, ApiError> = opened.iter().map(Opened::confined_arg).collect();
    let args = match args {
        Ok(args) => args,
        Err(refused) => {
            job.finish(JobState::Failed, refused.to_string(), false);
            let _ = delivery.give(&job, Err(refused));
            return;
        }
    };
    let expires = deadline(now_ms(), plan.expires);
    let worker = service.clone();
    // The copy looks at this between files and between chunks, so a cancel stops it within a chunk.
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    let preparing = {
        let stop = stop.clone();
        tokio::spawn(async move { worker.prepare(args, expires, stop).await })
    };
    let watching = {
        let job = job.clone();
        tokio::spawn(async move {
            job.cancelled().await;
            flag.store(true, Ordering::Relaxed);
        })
    };
    let prepared = preparing.await.unwrap_or_else(|_| {
        Err(Error::Bug(
            "the send's preparing did not finish".to_string(),
        ))
    });
    watching.abort();
    let cancelled = |what: &str| {
        job.finish(JobState::Cancelled, what.to_string(), false);
    };
    if stop.load(Ordering::Relaxed) {
        // The copy stopped and closed its store on the way out, or the preparing finished before
        // the cancel was seen and the send it made is stopped here, so either way the store is
        // closed and its folder removed and not left for a later command.
        if let Ok(outgoing) = prepared {
            let _ = outgoing.provider.shutdown().await;
        }
        cancelled("the job was cancelled");
        let gone = ApiError::Unavailable("the send was cancelled".to_string());
        let _ = delivery.give(&job, Err(gone));
        return;
    }
    let mut outgoing = match prepared {
        Ok(outgoing) => outgoing,
        Err(error) => {
            job.finish(JobState::Failed, message_of(&error, ""), false);
            let _ = delivery.give(&job, Err(ApiError::from_core(&error, "")));
            return;
        }
    };
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
    if !delivery.give(&job, Ok(response)) {
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
