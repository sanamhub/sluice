//! The tasks that run a send and a receive once they are started, until they end or are cancelled.

use std::sync::Arc;

use sluice_core::Error;
use sluice_core::engine::{Keep, Outgoing, Pins, ReceiveOptions, StopEnd, receive_checked};
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

use crate::error::message_of;
use crate::jobs::Job;
use crate::proto::JobState;
use crate::roots::Opened;

/// Hands every event the engine reports to the job, until the engine drops its sender.
async fn pump(job: Arc<Job>, mut events: UnboundedReceiver<sluice_core::engine::Event>) {
    while let Some(event) = events.recv().await {
        job.observe(&event);
    }
}

pub(super) async fn run_send(
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

pub(super) async fn run_receive(
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
