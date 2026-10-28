//! Several receives through the queue: three run at once by default, high before normal before
//! low, and a network failure waits as the queue says. Each job is one `receive` on the same
//! runtime, and the partial store of a retried job is the one the last attempt left.

use std::{
    collections::{BTreeMap, VecDeque},
    time::{Duration, Instant},
};

use sluice_core::{
    Error, ErrorKind,
    engine::{ReceiveOptions, receive},
    queue::{Action, JobId, Priority, Queue, Work},
    ticket::{Body, Ticket},
};
use tokio::task::JoinSet;

use crate::{events, output::Output};

/// What a finished task says: its job, the bytes or error, and whether its events printed.
type Finished = (JobId, Result<u64, Error>, Result<(), Error>);

/// The queue of one `recv` command. The tickets are decoded first, so a bad one is refused before
/// any transfer starts, and every job writes into the same output folder.
pub fn run(
    output: &Output,
    tickets: &[String],
    priority: Priority,
    options: ReceiveOptions,
) -> Result<u8, Error> {
    let mut queue = Queue::new();
    let mut texts = BTreeMap::new();
    let mut pending = VecDeque::new();
    // Two tickets with one root would share a partial store, and the job that failed to load it
    // would remove the store the other job is using, so a command that names one transfer twice is
    // refused before anything starts.
    let mut roots: BTreeMap<[u8; 32], usize> = BTreeMap::new();
    for (index, text) in tickets.iter().enumerate() {
        let ticket = Ticket::decode(text)?;
        let Body::Static { root, .. } = ticket.body() else {
            return Err(Error::Policy(
                "a live ticket is received on its own, not with other tickets".to_string(),
            ));
        };
        if let Some(first) = roots.insert(*root, index) {
            return Err(Error::Policy(format!(
                "ticket {} and ticket {} are the same transfer, which one partial store cannot hold \
                 twice; give one of them",
                first + 1,
                index + 1
            )));
        }
        let (id, actions) = queue.add(
            Work::Receive {
                ticket,
                path: options.out.clone(),
            },
            priority,
        )?;
        texts.insert(id, text.clone());
        pending.extend(actions);
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| Error::Storage(format!("cannot start the transfer runtime: {e}")))?;
    runtime.block_on(drive(queue, texts, options, *output, pending))
}

async fn drive(
    mut queue: Queue,
    mut texts: BTreeMap<JobId, String>,
    options: ReceiveOptions,
    output: Output,
    mut pending: VecDeque<Action>,
) -> Result<u8, Error> {
    let started = Instant::now();
    let mut tasks: JoinSet<Finished> = JoinSet::new();
    let mut wake_at: Option<u64> = None;
    // The last state of every job that is not a success, by job, so a job that fails and then
    // finishes on a retry does not leave the command's exit code behind.
    let mut failures: BTreeMap<JobId, ErrorKind> = BTreeMap::new();
    loop {
        while let Some(action) = pending.pop_front() {
            match action {
                Action::Start(id) => spawn(&mut tasks, &mut texts, &options, output, id)?,
                Action::Stop(_) => {}
                Action::Save => {}
                Action::Wake { at_ms } => wake_at = at_ms,
            }
        }
        if tasks.is_empty() && queue.jobs().is_empty() {
            break;
        }
        let wait = wake_at
            .filter(|at| *at > elapsed_ms(started))
            .map(|at| Duration::from_millis(at - elapsed_ms(started)));
        // Only a retry is left, and its time has not come: wait for it, and stop if there is
        // nothing left to wait for.
        if wait.is_none() && tasks.is_empty() {
            if !wake_up(&mut queue, &mut pending, wake_at, started).await? {
                break;
            }
            wake_at = None;
            continue;
        }
        match step(&mut tasks, wait).await {
            Step::Joined(joined) => pending.extend(settle(
                joined,
                &mut queue,
                &mut failures,
                &mut texts,
                started,
            )?),
            Step::Due => {
                pending.extend(queue.tick(elapsed_ms(started))?);
                wake_at = None;
            }
        }
    }
    Ok(failures
        .values()
        .next()
        .map_or(0, |kind| crate::exit::code_of(*kind)))
}

/// One wait of the loop: a task that finished, or the retry time coming due.
enum Step {
    Joined(Option<Result<Finished, tokio::task::JoinError>>),
    Due,
}

/// Waits for whichever comes first: a task finishing, or `wait` running out. With no tasks left the
/// task branch is not polled, because it would answer `None` at once and spin.
async fn step(tasks: &mut JoinSet<Finished>, wait: Option<Duration>) -> Step {
    match wait {
        Some(wait) => tokio::select! {
            joined = tasks.join_next(), if !tasks.is_empty() => Step::Joined(joined),
            _ = tokio::time::sleep(wait) => Step::Due,
        },
        None => Step::Joined(tasks.join_next().await),
    }
}

/// Waits for the next retry to come due and lets the queue start it, or says there is nothing
/// left to wait for.
async fn wake_up(
    queue: &mut Queue,
    pending: &mut VecDeque<Action>,
    at: Option<u64>,
    started: Instant,
) -> Result<bool, Error> {
    let Some(at) = at else {
        if queue.jobs().is_empty() {
            return Ok(false);
        }
        return Err(Error::Bug(
            "the queue waits without a time and without a task".to_string(),
        ));
    };
    tokio::time::sleep(Duration::from_millis(
        at.saturating_sub(elapsed_ms(started)),
    ))
    .await;
    pending.extend(queue.tick(elapsed_ms(started))?);
    Ok(true)
}

/// Starts one job's receive on the runtime. Its events print as they arrive, and the task hands
/// back the id, the bytes or error, and whether printing failed.
fn spawn(
    tasks: &mut JoinSet<Finished>,
    texts: &mut BTreeMap<JobId, String>,
    options: &ReceiveOptions,
    output: Output,
    id: JobId,
) -> Result<(), Error> {
    // The ticket stays in the map: the queue may start this job again after a network failure,
    // and the retry reads the same ticket the first attempt did.
    let Some(text) = texts.get(&id).cloned() else {
        return Err(Error::Bug(format!(
            "the queue started job {id}, which has no ticket"
        )));
    };
    let options = options.clone();
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let printer = events::printer(output, rx, "receiving");
    tasks.spawn(async move {
        let result = receive(&text, &options, &tx).await;
        drop(tx);
        let printed = match printer.await {
            Ok(inner) => inner,
            Err(e) => Err(Error::Bug(format!("the event printer stopped: {e}"))),
        };
        let bytes = match result {
            Ok(received) => Ok(received.bytes),
            Err(e) => Err(e),
        };
        (id, bytes, printed)
    });
    Ok(())
}

/// One finished task: report it to the queue as done or failed, and return the actions that follow.
fn settle(
    joined: Option<Result<Finished, tokio::task::JoinError>>,
    queue: &mut Queue,
    failures: &mut BTreeMap<JobId, ErrorKind>,
    texts: &mut BTreeMap<JobId, String>,
    started: Instant,
) -> Result<Vec<Action>, Error> {
    let Some(joined) = joined else {
        return Ok(Vec::new());
    };
    let (id, result, printed) =
        joined.map_err(|e| Error::Bug(format!("a receive task stopped: {e}")))?;
    printed?;
    // A job that ends without an error is done, and the ticket is no longer needed. A job that
    // fails may be started again with the ticket it was given.
    match result {
        Ok(_) => {
            failures.remove(&id);
            texts.remove(&id);
            queue.finished(id)
        }
        Err(e) => {
            failures.insert(id, e.kind());
            queue.failed(id, e.kind(), elapsed_ms(started))
        }
    }
}

fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis() as u64
}
