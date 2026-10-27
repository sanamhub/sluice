//! Several receives through the queue: three run at once by default, high before normal before
//! low, and a network failure waits as the queue says. Each job is one `receive` on the same
//! runtime, and the partial store of a retried job is the one the last attempt left.

use std::{
    collections::BTreeMap,
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
    let mut pending = Vec::new();
    for text in tickets {
        let ticket = Ticket::decode(text)?;
        if !matches!(ticket.body(), Body::Static { .. }) {
            return Err(Error::Policy(
                "a live ticket is received on its own, not with other tickets".to_string(),
            ));
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
    mut pending: Vec<Action>,
) -> Result<u8, Error> {
    let started = Instant::now();
    let mut tasks: JoinSet<Finished> = JoinSet::new();
    let mut wake_at: Option<u64> = None;
    let mut failure: Option<ErrorKind> = None;
    loop {
        while let Some(action) = pending.pop() {
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
        match wait {
            Some(wait) => {
                tokio::select! {
                    joined = tasks.join_next(), if !tasks.is_empty() => {
                        pending.extend(settle(joined, &mut queue, &mut failure, started)?);
                    }
                    _ = tokio::time::sleep(wait) => {
                        pending.extend(queue.tick(elapsed_ms(started))?);
                        wake_at = None;
                    }
                }
            }
            None if !tasks.is_empty() => {
                let joined = tasks.join_next().await;
                pending.extend(settle(joined, &mut queue, &mut failure, started)?);
            }
            // Only a retry is left, and its time has not come.
            None => {
                if !wake_up(&mut queue, &mut pending, wake_at, started).await? {
                    break;
                }
                wake_at = None;
            }
        }
    }
    Ok(failure.map_or(0, crate::exit::code_of))
}

/// Waits for the next retry to come due and lets the queue start it, or says there is nothing
/// left to wait for.
async fn wake_up(
    queue: &mut Queue,
    pending: &mut Vec<Action>,
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
    let Some(text) = texts.remove(&id) else {
        return Err(Error::Bug(format!("the queue started job {id} twice")));
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
    failure: &mut Option<ErrorKind>,
    started: Instant,
) -> Result<Vec<Action>, Error> {
    let Some(joined) = joined else {
        return Ok(Vec::new());
    };
    let (id, result, printed) =
        joined.map_err(|e| Error::Bug(format!("a receive task stopped: {e}")))?;
    printed?;
    match result {
        Ok(_) => queue.finished(id),
        Err(e) => {
            *failure = Some(e.kind());
            queue.failed(id, e.kind(), elapsed_ms(started))
        }
    }
}

fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis() as u64
}
