//! Every command that makes no sense is a policy error and changes nothing.

use std::path::PathBuf;

use super::super::{Action, JobId, MAX_JOBS, MAX_PATH_UNITS, Priority, Queue, Work};
use super::{add, queue_of, send};
use crate::{Error, ErrorKind};

/// A queue with a job in every state, with one slot. The ids are the constants below.
fn every_state() -> Queue {
    let mut queue = queue_of(7);
    queue.set_slots(1).expect("slots");
    // Jobs 1 to 3 run. With one slot, nothing starts until all three are gone.
    queue.finished(JobId(2)).expect("2");
    queue.finished(JobId(3)).expect("3");
    // Pausing 1 starts 4, a network failure of 4 starts 5, and so on.
    queue.pause(JobId(1)).expect("pause 1");
    queue.failed(JobId(4), ErrorKind::Network, 0).expect("4");
    queue.finished(JobId(5)).expect("5");
    queue.failed(JobId(6), ErrorKind::Storage, 0).expect("6");
    queue.cancel(JobId(7)).expect("7");
    // Nothing runs now: 8 starts and 9 waits.
    add(&mut queue, "active", Priority::Normal);
    add(&mut queue, "waiting", Priority::Normal);
    queue
}

const ACTIVE: JobId = JobId(8);
const WAITING: JobId = JobId(9);
const PAUSED: JobId = JobId(1);
const RETRYING: JobId = JobId(4);
const DONE: JobId = JobId(5);
const FAILED: JobId = JobId(6);
const CANCELLED: JobId = JobId(7);
const UNKNOWN: JobId = JobId(77);
const HISTORY: [JobId; 3] = [DONE, FAILED, CANCELLED];

/// Runs `command` on a queue with every state and checks that it is refused with a policy error
/// whose message names the job, and that the queue is as it was.
fn refused(id: JobId, command: impl Fn(&mut Queue) -> Result<Vec<Action>, Error>) {
    let mut queue = every_state();
    let before = queue.clone();
    match command(&mut queue) {
        Err(Error::Policy(message)) => {
            assert!(message.contains(&id.to_string()), "{id}: {message}")
        }
        other => panic!("{id}: expected a policy error, got {other:?}"),
    }
    assert_eq!(queue, before, "{id}: the queue changed");
}

#[test]
fn the_fixture_has_a_job_in_every_state() {
    use super::super::State;
    let queue = every_state();
    let states: Vec<State> = [ACTIVE, WAITING, PAUSED, RETRYING, DONE, FAILED, CANCELLED]
        .iter()
        .map(|id| queue.job(*id).expect("job").state())
        .collect();
    assert_eq!(
        states,
        [
            State::Active,
            State::Waiting,
            State::Paused,
            State::Retrying { at_ms: 1000 },
            State::Done,
            State::Failed(ErrorKind::Storage),
            State::Cancelled
        ]
    );
}

#[test]
fn every_command_on_an_id_the_queue_does_not_have() {
    refused(UNKNOWN, |q| q.pause(UNKNOWN));
    refused(UNKNOWN, |q| q.resume(UNKNOWN));
    refused(UNKNOWN, |q| q.cancel(UNKNOWN));
    refused(UNKNOWN, |q| q.set_priority(UNKNOWN, Priority::High));
    refused(UNKNOWN, |q| q.progress(UNKNOWN));
    refused(UNKNOWN, |q| q.finished(UNKNOWN));
    refused(UNKNOWN, |q| q.failed(UNKNOWN, ErrorKind::Network, 0));
    refused(JobId(0), |q| q.pause(JobId(0)));
}

#[test]
fn every_command_on_a_job_in_the_history() {
    for id in HISTORY {
        refused(id, |q| q.pause(id));
        refused(id, |q| q.resume(id));
        refused(id, |q| q.cancel(id));
        refused(id, |q| q.set_priority(id, Priority::Low));
        refused(id, |q| q.progress(id));
        refused(id, |q| q.finished(id));
        refused(id, |q| q.failed(id, ErrorKind::Verify, 0));
    }
}

#[test]
fn pausing_a_paused_job_and_resuming_one_that_is_not_paused() {
    refused(PAUSED, |q| q.pause(PAUSED));
    for id in [ACTIVE, WAITING, RETRYING] {
        refused(id, |q| q.resume(id));
    }
}

#[test]
fn a_report_about_a_job_that_is_not_running() {
    for id in [WAITING, PAUSED, RETRYING] {
        refused(id, |q| q.progress(id));
        refused(id, |q| q.finished(id));
        refused(id, |q| q.failed(id, ErrorKind::Network, 0));
    }
}

#[test]
fn slots_out_of_range() {
    for slots in [0, 65, 200, u8::MAX] {
        let mut queue = every_state();
        let before = queue.clone();
        assert!(
            matches!(queue.set_slots(slots), Err(Error::Policy(_))),
            "{slots}"
        );
        assert_eq!(queue, before);
    }
}

fn refused_add(queue: &mut Queue, work: Work) -> String {
    let before = queue.clone();
    let Err(Error::Policy(message)) = queue.add(work, Priority::Normal) else {
        panic!("expected a policy error");
    };
    assert_eq!(*queue, before);
    message
}

#[test]
fn a_job_with_an_empty_path_or_one_too_long() {
    let mut queue = every_state();
    assert!(refused_add(&mut queue, send("")).contains("empty"));
    let long = "a".repeat(MAX_PATH_UNITS + 1);
    assert!(refused_add(&mut queue, send(&long)).contains("4097"));
    let longest = PathBuf::from("a".repeat(MAX_PATH_UNITS));
    assert!(
        queue
            .add(Work::Send { path: longest }, Priority::Low)
            .is_ok()
    );
}

#[test]
fn a_job_past_the_most_the_queue_takes() {
    let mut queue = Queue::new();
    for at in 0..MAX_JOBS {
        add(&mut queue, &format!("f{at}"), Priority::Normal);
    }
    assert!(refused_add(&mut queue, send("one more")).contains("1000"));
    // History does not count: once one is done, there is room again.
    queue.finished(JobId(1)).expect("finished");
    assert!(queue.add(send("now it fits"), Priority::Normal).is_ok());
}

#[test]
fn a_queue_that_has_used_every_id() {
    let mut queue = Queue::new();
    queue.next_id = u64::MAX;
    let message = refused_add(&mut queue, send("last"));
    assert!(message.contains("id"), "{message}");
}
