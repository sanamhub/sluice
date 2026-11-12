use super::*;

fn progress(done: u64) -> proto::Event {
    event(
        "job-1",
        Kind::Progress(proto::Progress {
            bytes_done: done,
            bytes_total: 0,
            bytes_per_second: 0,
        }),
    )
}

fn done() -> proto::Event {
    event(
        "job-1",
        Kind::Done(proto::Done {
            state: proto::JobState::Done as i32,
            message: String::new(),
            peer_close_reason: None,
        }),
    )
}

#[tokio::test]
async fn a_watcher_that_never_reads_gets_a_bounded_buffer_and_a_count_of_what_it_missed() {
    let hub = Hub::default();
    let mut watcher = hub.watch("job-1").unwrap();
    let sent = WATCH_BUFFER as u64 + 744;
    for n in 0..sent {
        hub.publish(&progress(n));
        assert!(watcher.waiting() <= WATCH_BUFFER + 2, "the buffer grew");
    }
    hub.finish(&done());
    assert!(watcher.waiting() <= WATCH_BUFFER + 2);

    let mut kept = 0u64;
    let mut dropped = 0u64;
    let mut last = None;
    while let Some(next) = watcher.next().await {
        match next.event {
            Some(Kind::Progress(p)) => {
                assert_eq!(
                    p.bytes_done, kept,
                    "events arrive in order, with no hole before the gap"
                );
                kept += 1;
            }
            Some(Kind::Dropped(gap)) => dropped += gap.count,
            other => last = other,
        }
    }
    assert_eq!(kept, WATCH_BUFFER as u64);
    assert_eq!(dropped, sent - kept);
    assert!(
        matches!(last, Some(Kind::Done(_))),
        "the end is still delivered"
    );
}

#[tokio::test]
async fn a_reader_that_keeps_up_sees_every_event_and_no_gap() {
    let hub = Hub::default();
    let mut watcher = hub.watch("job-1").unwrap();
    for n in 0..1000 {
        hub.publish(&progress(n));
        let got = watcher.next().await.unwrap();
        assert!(matches!(got.event, Some(Kind::Progress(p)) if p.bytes_done == n));
    }
}

#[tokio::test]
async fn a_watcher_that_joins_after_the_end_gets_the_last_event_and_then_the_end() {
    let hub = Hub::default();
    hub.finish(&done());
    let mut late = hub.watch("job-1").unwrap();
    assert!(matches!(
        late.next().await.unwrap().event,
        Some(Kind::Done(_))
    ));
    assert!(late.next().await.is_none());
}

#[tokio::test]
async fn a_job_has_a_limit_on_its_watchers() {
    let hub = Hub::default();
    let held: Vec<_> = (0..MAX_WATCHERS)
        .map(|_| hub.watch("job-1").unwrap())
        .collect();
    assert!(matches!(hub.watch("job-1"), Err(crate::ApiError::Busy(_))));
    drop(held);
    assert!(
        hub.watch("job-1").is_ok(),
        "watchers that went away free their places"
    );
}
