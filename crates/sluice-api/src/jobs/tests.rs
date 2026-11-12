use super::*;

fn table() -> Table {
    Table::default()
}

#[test]
fn jobs_are_listed_in_the_order_they_started_and_filtered_by_state() {
    let jobs = table();
    let first = jobs.start(JobKind::Send, None).unwrap();
    let second = jobs.start(JobKind::Receive, None).unwrap();
    first.finish(JobState::Done, String::new(), false);
    let ids: Vec<String> = jobs
        .list(JobState::Unspecified)
        .into_iter()
        .map(|j| j.id)
        .collect();
    assert_eq!(ids, [first.id.clone(), second.id.clone()]);
    let running = jobs.list(JobState::Running);
    assert_eq!(running.len(), 1);
    assert_eq!(running[0].id, second.id);
}

#[test]
fn a_daemon_runs_a_bounded_number_of_jobs_and_a_finished_one_frees_its_place() {
    let jobs = table();
    let started: Vec<_> = (0..MAX_RUNNING)
        .map(|_| jobs.start(JobKind::Send, None).unwrap())
        .collect();
    assert!(matches!(
        jobs.start(JobKind::Send, None),
        Err(ApiError::Busy(_))
    ));
    started[0].finish(JobState::Cancelled, String::new(), false);
    assert!(jobs.start(JobKind::Send, None).is_ok());
}

#[test]
fn the_table_forgets_the_oldest_finished_jobs_and_never_a_running_one() {
    let jobs = table();
    let running = jobs.start(JobKind::Send, None).unwrap();
    for _ in 0..MAX_FINISHED + 10 {
        let job = jobs.start(JobKind::Receive, None).unwrap();
        job.finish(JobState::Done, String::new(), false);
    }
    let listed = jobs.list(JobState::Unspecified);
    assert!(
        listed.len() <= MAX_FINISHED + 1,
        "{} jobs kept",
        listed.len()
    );
    assert!(listed.iter().any(|job| job.id == running.id));
}

#[tokio::test]
async fn cancel_wakes_the_task_and_says_whether_the_job_was_running() {
    let jobs = table();
    let job = jobs.start(JobKind::Receive, None).unwrap();
    let waiting = {
        let job = job.clone();
        tokio::spawn(async move { job.cancelled().await })
    };
    assert!(job.cancel());
    waiting.await.unwrap();
    job.finish(JobState::Cancelled, String::new(), true);
    assert!(!job.cancel(), "a finished job was not running");
    let summary = job.summary();
    assert_eq!(summary.state, JobState::Cancelled as i32);
    assert!(summary.resumable);
}

#[tokio::test]
async fn the_engine_events_become_the_events_a_watcher_reads() {
    let jobs = table();
    let job = jobs.start(JobKind::Receive, Some("ab".repeat(32))).unwrap();
    let mut watcher = job.watch().unwrap();
    job.observe(&Engine::Progress {
        bytes: 5,
        total: 10,
    });
    job.observe(&Engine::Verified { bytes: 10 });
    job.finish(JobState::Done, String::new(), false);
    let mut names = Vec::new();
    while let Some(next) = watcher.next().await {
        names.push(match next.event.unwrap() {
            Kind::Progress(p) => format!("progress {}/{}", p.bytes_done, p.bytes_total),
            Kind::Verified(v) => format!("verified {}", v.root.len()),
            Kind::Done(d) => format!("done {}", d.state),
            _ => "other".to_string(),
        });
    }
    assert_eq!(names, ["progress 5/10", "verified 64", "done 3"]);
    assert_eq!(job.summary().bytes_done, 5);
}
