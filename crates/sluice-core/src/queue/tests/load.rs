//! What a load refuses, and what a restart keeps of the ids and the timer.

use std::fs;
use std::path::PathBuf;

use tempfile::TempDir;

use super::super::save::{decode, encode, file_in};
use super::super::{Action, HISTORY, JobId, MAX_JOBS, Priority, Queue, State, Work};
use super::save::{home, live_body};
use super::{add, queue_of};
use crate::ticket::Ticket;
use crate::{Error, ErrorKind, Home};

/// Writes `bytes` as the queue file, loads it, and checks the load fails with a storage error that
/// names the file and says `why`, and that the file is as it was.
fn refused(bytes: &[u8], why: &str) {
    let (dir, home) = home();
    let path = file_in(dir.path());
    fs::write(&path, bytes).expect("write");
    let Err(Error::Storage(message)) = Queue::load(&home) else {
        panic!("expected a storage error for: {why}");
    };
    assert!(message.contains(&path.display().to_string()), "{message}");
    assert!(message.contains(why), "{why} not in: {message}");
    assert_eq!(
        fs::read(&path).expect("still there"),
        bytes,
        "the file changed"
    );
}

fn home_with(bytes: &[u8]) -> (TempDir, Home) {
    let (dir, home) = home();
    fs::write(file_in(dir.path()), bytes).expect("write");
    (dir, home)
}

/// Jobs 1 and 2 run, and job 3 is done.
fn base() -> Queue {
    let mut queue = queue_of(3);
    queue.finished(JobId(3)).expect("finished");
    queue
}

/// A queue changed by `change` and saved, which a load must refuse with `why`.
fn refused_queue(why: &str, change: impl FnOnce(&mut Queue)) {
    let mut queue = base();
    change(&mut queue);
    refused(&encode(&queue).expect("encode"), why);
}

#[test]
fn a_file_over_16_mib_is_refused() {
    let mut bytes = encode(&queue_of(1)).expect("encode");
    bytes.resize((16 << 20) + 1, 0);
    refused(&bytes, "over 16 MiB");
}

#[test]
fn an_unknown_version_is_refused() {
    let mut bytes = encode(&queue_of(1)).expect("encode");
    bytes[0] = 2;
    refused(&bytes, "version 2");
    refused(&[], "empty");
}

#[test]
fn bytes_that_do_not_decode_are_refused() {
    let good = encode(&queue_of(2)).expect("encode");
    refused(&good[..good.len() - 3], "does not decode");
    refused(&[1, 0xff, 0xff, 0xff], "does not decode");
    let mut long = good.clone();
    long.push(0);
    refused(&long, "bytes after the queue");
}

#[test]
fn a_queue_this_one_could_not_have_saved_is_refused() {
    refused_queue("slot count is 0", |q| q.slots = 0);
    refused_queue("slot count is 65", |q| q.slots = 65);
    refused_queue("next job id is 0", |q| q.next_id = 0);
    refused_queue("out of order or used twice", |q| q.next_id = 2);
    refused_queue("out of order or used twice", |q| {
        let copy = q.jobs[&JobId(1)].clone();
        q.history.push_back(copy);
    });
    refused_queue("a state the queue never leaves", |q| {
        q.jobs.get_mut(&JobId(1)).expect("job").state = State::Done;
    });
    refused_queue("a state the queue never leaves", |q| {
        q.history[0].state = State::Waiting;
    });
    refused_queue("a path of 0 units", |q| {
        q.jobs.get_mut(&JobId(1)).expect("job").work = Work::Send {
            path: PathBuf::new(),
        };
    });
}

/// Adds copies of job 1 under new ids, to the jobs or to the history, until there are `count`.
fn grow(queue: &mut Queue, count: usize, in_history: bool) {
    let template = queue.jobs[&JobId(1)].clone();
    while (if in_history {
        queue.history.len()
    } else {
        queue.jobs.len()
    }) < count
    {
        let mut job = template.clone();
        job.id = JobId(queue.next_id);
        queue.next_id += 1;
        if in_history {
            job.state = State::Cancelled;
            queue.history.push_back(job);
        } else {
            queue.jobs.insert(job.id, job);
        }
    }
}

#[test]
fn more_jobs_or_history_than_a_queue_keeps_is_refused() {
    let mut most = base();
    grow(&mut most, MAX_JOBS, false);
    grow(&mut most, HISTORY, true);
    decode(&encode(&most).expect("encode")).expect("the most a queue keeps loads");
    refused_queue("more jobs than a queue takes", |q| {
        grow(q, MAX_JOBS + 1, false)
    });
    refused_queue("more jobs than a queue takes", |q| {
        grow(q, HISTORY + 1, true)
    });
}

#[cfg(windows)]
#[test]
fn a_windows_path_of_an_odd_number_of_bytes_is_refused() {
    let mut queue = Queue::new();
    add(&mut queue, "ab", Priority::Normal);
    let bytes = encode(&queue).expect("encode");
    // The path is its length, 4, then "ab" in UTF-16LE. Drop its last byte and say it is 3 long.
    let path = [4, b'a', 0, b'b', 0];
    let at = bytes
        .windows(path.len())
        .position(|w| w == path)
        .expect("the path in the file");
    let mut odd = bytes[..at].to_vec();
    odd.extend_from_slice(&[3, b'a', 0, b'b']);
    odd.extend_from_slice(&bytes[at + path.len()..]);
    refused(&odd, "job 1 has a malformed path");
}

/// The states a job can be saved in, each with the counts of network failures in a row the queue
/// can leave it with there.
fn possible() -> Vec<(State, bool, std::ops::RangeInclusive<u32>)> {
    let mut all = vec![
        (State::Waiting, false, 0..=14),
        (State::Active, false, 0..=14),
        (State::Paused, false, 0..=14),
        (State::Retrying { at_ms: 7 }, false, 1..=14),
        (State::Done, true, 0..=14),
        (State::Cancelled, true, 0..=14),
        (State::Failed(ErrorKind::Network), true, 15..=15),
    ];
    for kind in [
        ErrorKind::Verify,
        ErrorKind::Storage,
        ErrorKind::Policy,
        ErrorKind::Cancelled,
        ErrorKind::Bug,
    ] {
        all.push((State::Failed(kind), true, 0..=14));
    }
    all
}

fn with_failures(state: State, in_history: bool, failures: u32) -> Queue {
    let mut queue = base();
    let job = if in_history {
        &mut queue.history[0]
    } else {
        queue.jobs.get_mut(&JobId(1)).expect("job")
    };
    job.state = state;
    job.failures = failures;
    queue
}

#[test]
fn a_load_takes_exactly_the_failure_counts_the_queue_can_leave_in_each_state() {
    for (state, in_history, counts) in possible() {
        for failures in [0, 1, 13, 14, 15, 16, u32::MAX] {
            let bytes = encode(&with_failures(state, in_history, failures)).expect("encode");
            if counts.contains(&failures) {
                decode(&bytes).unwrap_or_else(|why| panic!("{state:?} {failures}: {why}"));
            } else {
                refused(&bytes, "network failures in a row");
            }
        }
    }
}

#[test]
fn the_next_id_survives_a_restart_after_the_highest_has_left_the_history() {
    let mut queue = Queue::new();
    for at in 1..=101 {
        add(&mut queue, &format!("f{at}"), Priority::Normal);
    }
    queue.cancel(JobId(101)).expect("cancel");
    for id in 1..=100 {
        queue.cancel(JobId(id)).expect("cancel");
    }
    assert!(queue.job(JobId(101)).is_none(), "pushed out of the history");
    let (_dir, home) = home();
    queue.save(&home).expect("save");
    let (mut loaded, _) = Queue::load(&home).expect("load");
    assert_eq!(add(&mut loaded, "next", Priority::Normal), JobId(102));
}

#[test]
fn a_load_asks_for_no_timer_because_no_job_is_left_waiting_to_retry() {
    let mut queue = queue_of(2);
    queue
        .failed(JobId(1), ErrorKind::Network, 0)
        .expect("failed");
    let (_dir, home) = home();
    queue.save(&home).expect("save");
    let (loaded, actions) = Queue::load(&home).expect("load");
    assert!(
        actions.iter().all(|a| !matches!(a, Action::Wake { .. })),
        "{actions:?}"
    );
    assert_eq!(loaded.job(JobId(1)).expect("job").state(), State::Active);
}

#[test]
fn a_ticket_that_does_not_decode_is_refused_without_quoting_it() {
    let mut queue = Queue::new();
    let ticket = Ticket::new([1; 32], vec![], None, None, live_body()).expect("ticket");
    let text = ticket.encode();
    let work = Work::Receive {
        ticket,
        path: PathBuf::from("into"),
    };
    queue.add(work, Priority::Normal).expect("add");
    let mut bytes = encode(&queue).expect("encode");
    // The ticket's text is in the file as it is. Make one of its characters an `I`, which the
    // alphabet leaves out.
    let at = bytes
        .windows(text.len())
        .position(|w| w == text.as_bytes())
        .expect("the ticket in the file");
    bytes[at + 20] = b'I';
    refused(&bytes, "bad ticket");
    let message = Queue::load(&home_with(&bytes).1).unwrap_err().to_string();
    assert!(!message.contains(&text[10..30]), "{message}");
}

/// Fails the active job `id` on the network `times` times from `now`, ticking to the end of each
/// wait, which starts it again. Returns the time after the last tick.
fn fail_on_the_network(queue: &mut Queue, id: JobId, times: u32, mut now: u64) -> u64 {
    for _ in 0..times {
        let actions = queue.failed(id, ErrorKind::Network, now).expect("failed");
        let Some(&Action::Wake { at_ms: Some(at_ms) }) = actions.last() else {
            panic!("no timer in {actions:?}");
        };
        assert_eq!(
            queue.tick(at_ms).expect("tick"),
            [Action::Start(id), Action::Save]
        );
        now = at_ms;
    }
    now
}

#[test]
fn the_failure_counts_commands_leave_load_back_as_they_were() {
    let (_dir, home) = home();
    let mut queue = Queue::new();
    let failed = add(&mut queue, "failed", Priority::Normal);
    let done = add(&mut queue, "done", Priority::Normal);
    let cancelled = add(&mut queue, "cancelled", Priority::Normal);

    let now = fail_on_the_network(&mut queue, failed, 14, 0);
    queue.failed(failed, ErrorKind::Network, now).expect("15th");
    let now = fail_on_the_network(&mut queue, done, 14, now);
    queue.finished(done).expect("finished");
    fail_on_the_network(&mut queue, cancelled, 14, now);
    queue.cancel(cancelled).expect("cancel");

    let want = [
        (failed, State::Failed(ErrorKind::Network), 15),
        (done, State::Done, 14),
        (cancelled, State::Cancelled, 14),
    ];
    let seen = |queue: &Queue| {
        want.map(|(id, _, _)| {
            let job = queue.job(id).expect("job");
            (id, job.state(), job.network_failures())
        })
    };
    assert_eq!(seen(&queue), want, "what the commands left");
    queue.save(&home).expect("save");
    let (loaded, actions) = Queue::load(&home).expect("load");
    assert_eq!(seen(&loaded), want, "what the load gave back");
    assert_eq!(loaded, queue);
    assert!(actions.is_empty(), "{actions:?}");
}
