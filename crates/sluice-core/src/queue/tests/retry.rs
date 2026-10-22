//! Only network failures retry, with waits that double up to a cap.

use super::super::{Action, JobId, Priority, Queue, State, backoff_ms};
use super::{add, queue_of, starts, state};
use crate::ErrorKind;

#[test]
fn the_waits_double_from_1_s_and_stop_at_5_minutes() {
    let waits: Vec<u64> = (1..=9).map(backoff_ms).collect();
    assert_eq!(
        waits,
        [
            1000, 2000, 4000, 8000, 16_000, 32_000, 64_000, 128_000, 256_000
        ]
    );
    assert_eq!(backoff_ms(10), 300_000, "512 s is over the cap");
    for failures in [11, 20, 21, 64, 1000, u32::MAX] {
        assert_eq!(backoff_ms(failures), 300_000, "{failures}");
    }
    assert_eq!(
        backoff_ms(0),
        1000,
        "never asked for, and still the first wait"
    );
}

/// Fails the active job `id` on the network at `now`, then ticks to the end of its wait, which
/// starts it again. Returns the time after the tick.
fn fail_and_retry(queue: &mut Queue, id: JobId, now: u64) -> u64 {
    let actions = queue.failed(id, ErrorKind::Network, now).expect("failed");
    let Some(&Action::Wake { at_ms }) = actions.last() else {
        panic!("no timer in {actions:?}");
    };
    assert_eq!(
        queue.tick(at_ms).expect("tick"),
        [Action::Start(id), Action::Save]
    );
    at_ms
}

#[test]
fn a_network_failure_waits_its_turn_and_the_retry_is_due_on_the_timer() {
    let mut queue = Queue::new();
    let id = add(&mut queue, "a", Priority::Normal);
    let mut now = 50;
    for (failure, wait) in [(1, 1000), (2, 2000), (3, 4000)] {
        let actions = queue.failed(id, ErrorKind::Network, now).expect("failed");
        assert_eq!(
            actions,
            [Action::Save, Action::Wake { at_ms: now + wait }],
            "failure {failure}"
        );
        assert_eq!(queue.job(id).expect("job").network_failures(), failure);
        assert_eq!(
            queue.tick(now + wait - 1).expect("early"),
            [Action::Wake { at_ms: now + wait }]
        );
        assert_eq!(state(&queue, id.0), State::Retrying { at_ms: now + wait });
        now += wait;
        assert_eq!(
            queue.tick(now).expect("due"),
            [Action::Start(id), Action::Save]
        );
    }
}

#[test]
fn the_10th_network_failure_in_a_row_fails_the_job() {
    let mut queue = Queue::new();
    let id = add(&mut queue, "a", Priority::Normal);
    let mut now = 0;
    for _ in 1..=9 {
        now = fail_and_retry(&mut queue, id, now);
    }
    assert_eq!(queue.job(id).expect("job").network_failures(), 9);
    let actions = queue.failed(id, ErrorKind::Network, now).expect("failed");
    assert_eq!(actions, [Action::Save]);
    assert_eq!(state(&queue, id.0), State::Failed(ErrorKind::Network));
    assert_eq!(queue.job(id).expect("job").network_failures(), 10);
}

#[test]
fn progress_after_a_failure_starts_the_count_again() {
    let mut queue = Queue::new();
    let id = add(&mut queue, "a", Priority::Normal);
    let mut now = 0;
    for _ in 1..=9 {
        now = fail_and_retry(&mut queue, id, now);
    }
    assert_eq!(queue.progress(id).expect("progress"), [Action::Save]);
    assert_eq!(queue.job(id).expect("job").network_failures(), 0);
    assert_eq!(
        queue.progress(id).expect("again"),
        [],
        "nothing changed, so no save"
    );
    // Nine more in a row are allowed again, and the first wait is 1 s again.
    let actions = queue.failed(id, ErrorKind::Network, now).expect("failed");
    assert_eq!(actions.last(), Some(&Action::Wake { at_ms: now + 1000 }));
}

#[test]
fn every_other_kind_of_failure_fails_the_job_at_once() {
    for kind in [
        ErrorKind::Verify,
        ErrorKind::Storage,
        ErrorKind::Policy,
        ErrorKind::Bug,
        ErrorKind::Cancelled,
    ] {
        let mut queue = queue_of(4);
        let actions = queue.failed(JobId(1), kind, 0).expect("failed");
        assert_eq!(actions, [Action::Start(JobId(4)), Action::Save], "{kind:?}");
        assert_eq!(state(&queue, 1), State::Failed(kind));
        assert_eq!(queue.job(JobId(1)).expect("job").network_failures(), 0);
    }
}

#[test]
fn a_job_that_waits_to_retry_frees_its_slot_and_rejoins_its_line_in_its_old_place() {
    let mut queue = queue_of(5);
    queue.set_slots(1).expect("slots");
    queue.finished(JobId(2)).expect("finished");
    queue.finished(JobId(3)).expect("finished");
    // Job 1 still runs from before the slots were lowered.
    let actions = queue
        .failed(JobId(1), ErrorKind::Network, 0)
        .expect("failed");
    assert_eq!(starts(&actions), [4]);
    queue.tick(1000).expect("tick");
    assert_eq!(state(&queue, 1), State::Waiting);
    let actions = queue.finished(JobId(4)).expect("finished");
    assert_eq!(
        starts(&actions),
        [1],
        "ahead of job 5, which was added after it"
    );
}

#[test]
fn the_timer_is_set_for_the_earliest_retry() {
    let mut queue = queue_of(3);
    queue
        .failed(JobId(1), ErrorKind::Network, 10_000)
        .expect("failed");
    let actions = queue
        .failed(JobId(2), ErrorKind::Network, 500)
        .expect("failed");
    assert_eq!(actions.last(), Some(&Action::Wake { at_ms: 1500 }));
    // At 1500 job 2 is due, and job 1 is next at 11000.
    let actions = queue.tick(1500).expect("tick");
    assert_eq!(
        actions,
        [
            Action::Start(JobId(2)),
            Action::Save,
            Action::Wake { at_ms: 11_000 }
        ]
    );
    assert_eq!(
        queue.tick(11_000).expect("tick"),
        [Action::Start(JobId(1)), Action::Save]
    );
}

#[test]
fn a_wait_near_the_end_of_the_clock_saturates_instead_of_wrapping() {
    let mut queue = queue_of(1);
    let actions = queue
        .failed(JobId(1), ErrorKind::Network, u64::MAX - 10)
        .expect("failed");
    assert_eq!(actions.last(), Some(&Action::Wake { at_ms: u64::MAX }));
}

#[test]
fn pausing_a_job_that_waits_to_retry_keeps_its_count_and_resuming_puts_it_in_line() {
    let mut queue = queue_of(1);
    queue
        .failed(JobId(1), ErrorKind::Network, 0)
        .expect("failed");
    assert_eq!(queue.pause(JobId(1)).expect("pause"), [Action::Save]);
    assert_eq!(
        queue.tick(5000).expect("tick"),
        [],
        "a paused job is not due"
    );
    assert_eq!(
        queue.resume(JobId(1)).expect("resume"),
        [Action::Start(JobId(1)), Action::Save]
    );
    assert_eq!(queue.job(JobId(1)).expect("job").network_failures(), 1);
}
