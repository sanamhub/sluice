//! Two `sluice daemon` processes on loopback: one sends, one receives, and what each leaves behind
//! when a job ends, is stopped, or is read about in a log.

use crate::common::daemon::{at, ended_in, events_of, start};
use crate::common::data;
use sluice_api::proto::event::Event as Kind;
use sluice_api::proto::{
    JobState, ListJobsRequest, ShutdownRequest, StartReceiveRequest, StartSendRequest, Ticket,
};

struct Pair {
    _dir: tempfile::TempDir,
    outgoing: std::path::PathBuf,
    incoming: std::path::PathBuf,
    sender_home: std::path::PathBuf,
    receiver_home: std::path::PathBuf,
}

fn pair() -> Pair {
    let dir = tempfile::tempdir().unwrap();
    let outgoing = dir.path().join("outgoing");
    let incoming = dir.path().join("incoming");
    std::fs::create_dir(&outgoing).unwrap();
    std::fs::create_dir(&incoming).unwrap();
    Pair {
        sender_home: dir.path().join("sender-home"),
        receiver_home: dir.path().join("receiver-home"),
        outgoing,
        incoming,
        _dir: dir,
    }
}

fn send_of(pair: &Pair, name: &str) -> StartSendRequest {
    StartSendRequest {
        paths: vec![at(&pair.outgoing, name).unwrap()],
        ..Default::default()
    }
}

fn receive_of(pair: &Pair, ticket: &str, into: &str) -> StartReceiveRequest {
    StartReceiveRequest {
        ticket: Some(Ticket {
            text: ticket.to_string(),
        }),
        out: at(&pair.incoming, into),
        ..Default::default()
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_send_job_and_a_receive_job_between_two_daemons_finish_verified() {
    let pair = pair();
    let body = data(3 * 1024 * 1024 + 17, 9);
    std::fs::write(pair.outgoing.join("payload.bin"), &body).unwrap();
    let sender = start(&pair.sender_home, &[&pair.outgoing], "info");
    let receiver = start(&pair.receiver_home, &[&pair.incoming], "info");
    let (mut from, mut to) = (sender.client().await, receiver.client().await);

    let started = from
        .start_send(send_of(&pair, "payload.bin"))
        .await
        .unwrap()
        .into_inner();
    let ticket = started.ticket.unwrap().text;
    assert_eq!(started.bytes, body.len() as u64);
    let job = to
        .start_receive(receive_of(&pair, &ticket, "landing"))
        .await
        .unwrap()
        .into_inner()
        .job_id;

    let received = events_of(&mut to, &job).await;
    assert_eq!(ended_in(&received), JobState::Done as i32, "{received:?}");
    let verified = received.iter().find_map(|event| match &event.event {
        Some(Kind::Verified(verified)) => Some(verified.clone()),
        _ => None,
    });
    let verified = verified.expect("a verified event");
    assert_eq!(verified.bytes, body.len() as u64);
    assert_eq!(verified.root.len(), 64);
    assert_eq!(
        std::fs::read(pair.incoming.join("landing").join("payload.bin")).unwrap(),
        body
    );

    let sent = events_of(&mut from, &started.job_id).await;
    assert_eq!(ended_in(&sent), JobState::Done as i32, "{sent:?}");
    let listed = from
        .list_jobs(ListJobsRequest::default())
        .await
        .unwrap()
        .into_inner();
    assert_eq!(listed.jobs.len(), 1);
    assert_eq!(listed.jobs[0].state, JobState::Done as i32);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_ticket_never_appears_in_a_log_an_event_or_a_job_summary() {
    let pair = pair();
    std::fs::write(pair.outgoing.join("quiet.bin"), data(200 * 1024, 4)).unwrap();
    // The most that is logged, so a ticket that leaks into any line would show.
    let sender = start(&pair.sender_home, &[&pair.outgoing], "trace");
    let receiver = start(&pair.receiver_home, &[&pair.incoming], "trace");
    let (mut from, mut to) = (sender.client().await, receiver.client().await);

    let started = from
        .start_send(send_of(&pair, "quiet.bin"))
        .await
        .unwrap()
        .into_inner();
    let ticket = started.ticket.unwrap().text;
    let job = to
        .start_receive(receive_of(&pair, &ticket, "in"))
        .await
        .unwrap()
        .into_inner()
        .job_id;
    let received = events_of(&mut to, &job).await;
    let sent = events_of(&mut from, &started.job_id).await;
    assert_eq!(ended_in(&received), JobState::Done as i32);

    let listed = format!(
        "{:?}{:?}",
        from.list_jobs(ListJobsRequest::default())
            .await
            .unwrap()
            .into_inner(),
        to.list_jobs(ListJobsRequest::default())
            .await
            .unwrap()
            .into_inner()
    );
    let everything = format!(
        "{}{}{received:?}{sent:?}{listed}",
        sender.log_text(),
        receiver.log_text()
    );
    // The whole ticket, and the stretch of it that follows the fixed prefix, in case a line
    // cuts it short.
    assert!(!everything.contains(&ticket), "the ticket is in the output");
    let body = &ticket[ticket.len() / 2..ticket.len() / 2 + 24];
    assert!(
        !everything.contains(body),
        "a stretch of the ticket is in the output"
    );
    assert!(
        !sender.log_text().is_empty() && !receiver.log_text().is_empty(),
        "the logs were captured"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stop_cancels_a_running_receive_keeps_its_store_and_the_next_receive_resumes() {
    let pair = pair();
    // Large enough that a receive is still running when the first progress event arrives.
    let body = data(160 * 1024 * 1024, 3);
    std::fs::write(pair.outgoing.join("big.bin"), &body).unwrap();
    let sender = start(&pair.sender_home, &[&pair.outgoing], "info");
    let mut from = sender.client().await;
    let ticket = from
        .start_send(StartSendRequest {
            keep: true,
            ..send_of(&pair, "big.bin")
        })
        .await
        .unwrap()
        .into_inner()
        .ticket
        .unwrap()
        .text;

    let mut receiver = start(&pair.receiver_home, &[&pair.incoming], "info");
    let mut to = receiver.client().await;
    let job = to
        .start_receive(receive_of(&pair, &ticket, "big"))
        .await
        .unwrap()
        .into_inner()
        .job_id;
    let mut stream = to
        .watch_events(sluice_api::proto::WatchEventsRequest {
            job_id: job.clone(),
        })
        .await
        .unwrap()
        .into_inner();
    while let Some(next) = stream.message().await.unwrap() {
        if matches!(next.event.and_then(|e| e.event), Some(Kind::Progress(p)) if p.bytes_done > 0) {
            break;
        }
    }
    let stopping = to.shutdown(ShutdownRequest {}).await.unwrap().into_inner();
    assert_eq!(stopping.cancelled_jobs, 1);
    let mut last = None;
    while let Some(next) = stream.message().await.unwrap() {
        last = next.event;
    }
    let Some(Kind::Done(done)) = last.and_then(|e| e.event) else {
        panic!("the stream ends with a done event");
    };
    assert_eq!(done.state, JobState::Cancelled as i32, "{done:?}");
    assert!(receiver.wait_for_exit(), "a stopped daemon ends well");

    let kept = std::fs::read_dir(pair.incoming.join("big"))
        .unwrap()
        .filter_map(Result::ok)
        .any(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(".sluice-partial-")
        });
    assert!(kept, "the partial store stays beside the output");

    // A new daemon on the same home takes the same ticket and finishes it.
    let again = start(&pair.receiver_home, &[&pair.incoming], "info");
    let mut to = again.client().await;
    let job = to
        .start_receive(receive_of(&pair, &ticket, "big"))
        .await
        .unwrap()
        .into_inner()
        .job_id;
    let events = events_of(&mut to, &job).await;
    assert_eq!(ended_in(&events), JobState::Done as i32, "{events:?}");
    assert_eq!(
        std::fs::read(pair.incoming.join("big").join("big.bin")).unwrap(),
        body
    );
}

/// Each signal that stops a daemon ends it well, and the socket goes with it. Unix only: Windows
/// has no way to send a console event to a process from a test.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn sigint_sigterm_and_sighup_each_stop_the_daemon_and_remove_its_socket() {
    for signal in ["INT", "TERM", "HUP"] {
        let pair = pair();
        let mut daemon = start(&pair.sender_home, &[&pair.outgoing], "info");
        let socket = pair.sender_home.join("run").join("control.sock");
        assert!(socket.exists());
        let sent = std::process::Command::new("kill")
            .arg(format!("-{signal}"))
            .arg(daemon.id().to_string())
            .status()
            .unwrap();
        assert!(sent.success());
        assert!(daemon.wait_for_exit(), "SIG{signal} ends the daemon well");
        assert!(!socket.exists(), "SIG{signal} leaves no socket");
    }
}
