use super::*;
use crate::proto::{RootPath, StartReceiveRequest, StartSendRequest, Ticket};
use crate::roots::NO_ROOTS;

/// A service over a fresh home and the given roots. The home is the test's own, and nothing here
/// starts a transfer, so the engine's use of the user's home is never reached.
fn service(roots: &[std::path::PathBuf]) -> (tempfile::TempDir, Service) {
    let dir = tempfile::tempdir().unwrap();
    let home = Home::open(&dir.path().join("home")).unwrap();
    let roots = Roots::open(roots).unwrap();
    (dir, Service::new(home, roots, RelayModeChoice::Disabled))
}

fn at(root: &std::path::Path, path: &str) -> Option<RootPath> {
    Some(RootPath {
        root: root.to_str().unwrap().to_string(),
        path: path.to_string(),
    })
}

#[tokio::test]
async fn a_daemon_with_no_roots_refuses_every_send_and_every_receive() {
    let (_dir, service) = service(&[]);
    let send = service
        .start_send(StartSendRequest {
            paths: vec![RootPath::default()],
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert_eq!(send, ApiError::Refused(NO_ROOTS.to_string()));
    let receive = service
        .start_receive(StartReceiveRequest {
            ticket: Some(Ticket {
                text: "not even a ticket".to_string(),
            }),
            out: Some(RootPath::default()),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert_eq!(receive, ApiError::Refused(NO_ROOTS.to_string()));
    assert!(
        NO_ROOTS.contains("--root"),
        "the refusal says how to allow one"
    );
    assert!(
        service.list_jobs(&Default::default()).jobs.is_empty(),
        "a refusal makes no job"
    );
}

#[tokio::test]
async fn a_send_from_outside_every_root_is_refused_and_makes_no_job() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("root");
    std::fs::create_dir(&root).unwrap();
    let (_home, service) = service(std::slice::from_ref(&root));
    let outside = dir.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    let refused = service
        .start_send(StartSendRequest {
            paths: vec![at(&outside, "").unwrap()],
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert!(
        refused.to_string().contains("is not a root of this daemon"),
        "{refused}"
    );
    assert!(service.list_jobs(&Default::default()).jobs.is_empty());
}

#[tokio::test]
async fn a_new_daemon_lists_no_jobs_and_says_its_version() {
    let (_dir, service) = service(&[]);
    assert!(service.list_jobs(&Default::default()).jobs.is_empty());
    let version = service.version();
    assert_eq!(version.protocol, 1);
    assert_eq!(version.version, env!("CARGO_PKG_VERSION"));
}

#[tokio::test]
async fn a_job_that_does_not_exist_is_not_found_for_cancel_and_for_watch() {
    let (_dir, service) = service(&[]);
    let id = "job-9".to_string();
    let cancel = service.cancel_job(&proto::CancelJobRequest { job_id: id.clone() });
    assert!(matches!(cancel, Err(ApiError::NotFound(_))));
    let watch = service.watch_events(&proto::WatchEventsRequest { job_id: id });
    assert!(matches!(watch, Err(ApiError::NotFound(_))));
}

#[tokio::test]
async fn doctor_reports_the_checks_of_the_home() {
    let (_dir, service) = service(&[]);
    let report = service.doctor().await.unwrap();
    let names: Vec<&str> = report.checks.iter().map(|c| c.name.as_str()).collect();
    assert!(
        names.contains(&"home") && names.contains(&"key"),
        "{names:?}"
    );
}

#[tokio::test]
async fn shutdown_is_seen_by_whoever_waits_for_it_and_counts_no_jobs_when_none_run() {
    let (_dir, service) = service(&[]);
    let waiting = {
        let service = service.clone();
        tokio::spawn(async move { service.stop_requested().await })
    };
    assert_eq!(service.shutdown().cancelled_jobs, 0);
    waiting.await.unwrap();
}

#[tokio::test]
async fn a_watcher_that_cannot_keep_up_is_told_how_many_events_it_missed_and_its_buffer_stays_bounded()
 {
    let (_dir, service) = service(&[]);
    let job = service
        .inner
        .jobs
        .start(proto::JobKind::Receive, None)
        .unwrap();
    let mut slow = service
        .watch_events(&proto::WatchEventsRequest {
            job_id: job.id.clone(),
        })
        .unwrap();
    let sent = 5 * crate::WATCH_BUFFER as u64;
    for done in 0..sent {
        job.observe(&sluice_core::engine::Event::Progress {
            bytes: done,
            total: sent,
        });
        assert!(
            slow.waiting() <= crate::WATCH_BUFFER + 2,
            "the buffer grew past its bound"
        );
    }
    job.finish(JobState::Done, String::new(), false);

    let (mut kept, mut missed) = (0u64, 0u64);
    while let Some(next) = slow.next().await {
        match next.event {
            Some(proto::event::Event::Progress(_)) => kept += 1,
            Some(proto::event::Event::Dropped(gap)) => missed += gap.count,
            _ => {}
        }
    }
    assert_eq!(kept, crate::WATCH_BUFFER as u64);
    assert_eq!(missed, sent - kept, "every event is either read or counted");
}
