//! `sluice daemon` as a process: the interface it serves, the roots it holds to, the commands that
//! call it, and what it leaves out of its log.

use crate::common::daemon::{at, start};
use crate::common::{bare, sluice, stderr_text, stdout_text};
use sluice_api::NO_ROOTS;
use sluice_api::proto::{ListJobsRequest, RootPath, StartReceiveRequest, StartSendRequest, Ticket};
use tonic::Code;

fn folders() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let root = dir.path().join("root");
    std::fs::create_dir(&root).unwrap();
    (dir, home, root)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_daemon_on_a_temporary_home_lists_no_jobs_and_stops_when_asked() {
    let (_dir, home, root) = folders();
    let mut daemon = start(&home, &[&root], "info");
    let mut client = daemon.client().await;
    let jobs = client
        .list_jobs(ListJobsRequest::default())
        .await
        .unwrap()
        .into_inner();
    assert!(jobs.jobs.is_empty());
    let version = client
        .version(sluice_api::proto::VersionRequest {})
        .await
        .unwrap()
        .into_inner();
    assert_eq!(version.protocol, 1);
    drop(client);

    let status = sluice(&home).args(["daemon", "status"]).assert().success();
    let text = stdout_text(&status);
    assert!(text.contains("protocol 1, 0 jobs"), "{text}");
    let stopped = sluice(&home).args(["daemon", "stop"]).assert().success();
    assert!(stdout_text(&stopped).contains("0 running jobs were cancelled"));
    assert!(daemon.wait_for_exit(), "the daemon ends well");
    assert!(
        sluice_api::connect(&home).await.is_err(),
        "nothing answers now"
    );
}

#[test]
fn a_status_with_no_daemon_says_where_it_looked_and_how_to_start_one() {
    let (_dir, home, _root) = folders();
    let failed = sluice(&home).args(["daemon", "status"]).assert().code(3);
    let text = stderr_text(&failed);
    assert!(text.contains("no daemon answers at"), "{text}");
    assert!(text.contains("sluice daemon"), "{text}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_daemon_started_with_no_roots_refuses_every_send_and_receive_and_says_so() {
    let (_dir, home, _root) = folders();
    let daemon = start(&home, &[], "info");
    assert!(
        daemon.log_text().contains("no root folders"),
        "the start says it has none"
    );
    let mut client = daemon.client().await;
    let send = client
        .start_send(StartSendRequest {
            paths: vec![RootPath::default()],
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert_eq!(send.code(), Code::PermissionDenied);
    assert_eq!(send.message(), NO_ROOTS);
    let receive = client
        .start_receive(StartReceiveRequest {
            ticket: Some(Ticket {
                text: "anything".to_string(),
            }),
            out: Some(RootPath::default()),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert_eq!(receive.code(), Code::PermissionDenied);
    assert_eq!(receive.message(), NO_ROOTS);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_send_and_a_receive_outside_every_root_are_refused_with_the_reason() {
    let (dir, home, root) = folders();
    let outside = dir.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    std::fs::write(outside.join("secret.txt"), b"not yours").unwrap();
    let daemon = start(&home, &[&root], "info");
    let mut client = daemon.client().await;
    let send = client
        .start_send(StartSendRequest {
            paths: vec![at(&outside, "secret.txt").unwrap()],
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert_eq!(send.code(), Code::PermissionDenied);
    assert!(
        send.message().contains("is not a root of this daemon"),
        "{}",
        send.message()
    );
    // A `..` that comes back inside the root is refused as well, and is no way out of it.
    let climbing = client
        .start_send(StartSendRequest {
            paths: vec![at(&root, "../root").unwrap()],
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert_eq!(climbing.code(), Code::PermissionDenied);
    assert!(
        climbing.message().contains("rule 4"),
        "{}",
        climbing.message()
    );
    let jobs = client
        .list_jobs(ListJobsRequest::default())
        .await
        .unwrap()
        .into_inner();
    assert!(jobs.jobs.is_empty(), "a refusal starts no job");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_inspect_of_a_ticket_that_is_not_one_is_invalid_and_the_text_is_not_echoed() {
    let (_dir, home, root) = folders();
    let daemon = start(&home, &[&root], "info");
    let mut client = daemon.client().await;
    let secret = "sluice1thisisnotaticketbutitlooksliketheirstart";
    let refused = client
        .inspect_ticket(sluice_api::proto::InspectTicketRequest {
            ticket: Some(Ticket {
                text: secret.to_string(),
            }),
        })
        .await
        .unwrap_err();
    assert_eq!(refused.code(), Code::InvalidArgument);
    assert!(!refused.message().contains(secret), "{}", refused.message());
}

#[test]
fn the_daemon_flags_and_commands_are_in_the_help() {
    let help = bare().args(["daemon", "--help"]).assert().success();
    let text = stdout_text(&help);
    for word in ["--root", "--loopback", "status", "stop"] {
        assert!(text.contains(word), "{word} is missing from\n{text}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_watch_of_a_job_that_does_not_exist_is_not_found() {
    let (_dir, home, root) = folders();
    let daemon = start(&home, &[&root], "info");
    let mut client = daemon.client().await;
    let refused = client
        .watch_events(sluice_api::proto::WatchEventsRequest {
            job_id: "job-77".to_string(),
        })
        .await
        .unwrap_err();
    assert_eq!(refused.code(), Code::NotFound);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_one_shot_send_on_the_daemons_home_is_refused_while_it_runs() {
    let (dir, home, root) = folders();
    let _daemon = start(&home, &[&root], "info");
    let file = dir.path().join("one.bin");
    std::fs::write(&file, b"x").unwrap();
    let refused = sluice(&home).arg("send").arg(&file).assert().code(6);
    let text = stderr_text(&refused);
    assert!(text.contains("policy denied"), "{text}");
}
