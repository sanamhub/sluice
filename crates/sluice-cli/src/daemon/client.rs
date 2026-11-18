//! `sluice daemon status` and `sluice daemon stop`: a call each over the socket or pipe.

use std::time::{Duration, Instant};

use serde_json::json;
use sluice_api::proto::{
    JobKind, JobState, JobSummary, ListJobsRequest, ShutdownRequest, VersionRequest,
};
use sluice_core::{Error, Home};

use super::refused;
use crate::output::{Format, Output, bytes_text};

/// How long `stop` waits for the daemon to go away.
const GONE_WITHIN: Duration = Duration::from_secs(30);

/// How often `stop` looks for the daemon again.
const LOOK_EVERY: Duration = Duration::from_millis(50);

pub(super) async fn status(output: &Output) -> Result<u8, Error> {
    let mut client = sluice_api::connect(&Home::locate()?)
        .await
        .map_err(refused)?;
    let version = client
        .version(VersionRequest {})
        .await
        .map_err(answered)?
        .into_inner();
    let jobs = client
        .list_jobs(ListJobsRequest::default())
        .await
        .map_err(answered)?
        .into_inner()
        .jobs;
    let text = match output.format() {
        Format::Human => human(&version.version, version.protocol, &jobs),
        Format::Json => machine(&version.version, version.protocol, &jobs),
    };
    output.write_stdout(&text)?;
    Ok(0)
}

pub(super) async fn stop(output: &Output) -> Result<u8, Error> {
    let home = Home::locate()?;
    // A daemon with `--ui` writes this before it listens and removes it after it stops accepting.
    // `stop` must not answer before both are true: the file is part of what the daemon leaves, and
    // a caller that runs straight on would otherwise find it still there.
    let page = home.join(crate::ui::PORT_FILE);
    let page_was_here = page.exists();
    let mut client = sluice_api::connect(&home).await.map_err(refused)?;
    let stopped = client
        .shutdown(ShutdownRequest {})
        .await
        .map_err(answered)?
        .into_inner();
    drop(client);
    let started = Instant::now();
    // The daemon answers before it stops listening, so the end is the moment a connection is
    // refused. A connection that is accepted by the system and never answered, because the daemon
    // is closing, is not the end either. The page listener goes with the daemon, and its file is
    // removed after the listener: wait for that too when this daemon had one.
    while !gone(&home).await || (page_was_here && page.exists()) {
        if started.elapsed() > GONE_WITHIN {
            return Err(Error::Network(
                "the daemon was asked to stop and is still answering or clearing its port file"
                    .to_string(),
            ));
        }
        tokio::time::sleep(LOOK_EVERY).await;
    }
    let text = match output.format() {
        Format::Human => format!(
            "stopped; {} running {} cancelled\n",
            stopped.cancelled_jobs,
            if stopped.cancelled_jobs == 1 {
                "job was"
            } else {
                "jobs were"
            }
        ),
        Format::Json => format!(
            "{}\n",
            json!({ "stopped": true, "cancelled_jobs": stopped.cancelled_jobs })
        ),
    };
    output.write_stdout(&text)?;
    Ok(0)
}

/// Whether nothing is listening where the daemon was.
async fn gone(home: &std::path::Path) -> bool {
    matches!(
        tokio::time::timeout(LOOK_EVERY * 20, sluice_api::connect(home)).await,
        Ok(Err(_))
    )
}

/// A refusal from the daemon, as the kind of error that says what happened.
fn answered(status: tonic::Status) -> Error {
    let text = format!("the daemon answered: {}", status.message());
    match status.code() {
        tonic::Code::PermissionDenied | tonic::Code::InvalidArgument => Error::Policy(text),
        tonic::Code::Unavailable => Error::Network(text),
        _ => Error::Storage(text),
    }
}

fn kind_name(job: &JobSummary) -> String {
    JobKind::try_from(job.kind)
        .map_or("unknown", |kind| kind.as_str_name())
        .trim_start_matches("JOB_KIND_")
        .to_lowercase()
}

fn state_name(job: &JobSummary) -> String {
    JobState::try_from(job.state)
        .map_or("unknown", |state| state.as_str_name())
        .trim_start_matches("JOB_STATE_")
        .to_lowercase()
}

fn human(version: &str, protocol: u32, jobs: &[JobSummary]) -> String {
    let mut text = format!(
        "sluice daemon {version}, protocol {protocol}, {} jobs\n",
        jobs.len()
    );
    for job in jobs {
        text.push_str(&format!(
            "{} {} {} {} of {}\n",
            job.id,
            kind_name(job),
            state_name(job),
            bytes_text(job.bytes_done),
            bytes_text(job.bytes_total)
        ));
    }
    text
}

fn machine(version: &str, protocol: u32, jobs: &[JobSummary]) -> String {
    let jobs: Vec<_> = jobs
        .iter()
        .map(|job| {
            json!({
                "id": job.id,
                "kind": kind_name(job),
                "state": state_name(job),
                "bytes_total": job.bytes_total,
                "bytes_done": job.bytes_done,
                "receivers": job.receivers,
                "resumable": job.resumable,
            })
        })
        .collect();
    format!(
        "{}\n",
        json!({ "version": version, "protocol": protocol, "jobs": jobs })
    )
}
