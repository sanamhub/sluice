mod invalid;
mod load;
mod model;
mod order;
mod props;
mod retry;
mod save;

use std::path::PathBuf;

use super::{Action, JobId, Priority, Queue, State, Work};

fn send(name: &str) -> Work {
    Work::Send {
        path: PathBuf::from(name),
    }
}

/// Adds a job that sends `name` and returns its id.
fn add(queue: &mut Queue, name: &str, priority: Priority) -> JobId {
    queue.add(send(name), priority).expect("a valid job").0
}

/// A queue with `count` normal jobs, named after their ids.
fn queue_of(count: u64) -> Queue {
    let mut queue = Queue::new();
    for at in 1..=count {
        add(&mut queue, &format!("file-{at}"), Priority::Normal);
    }
    queue
}

fn state(queue: &Queue, id: u64) -> State {
    queue.job(JobId(id)).expect("a job").state()
}

/// The ids of the jobs that run, in id order.
fn running(queue: &Queue) -> Vec<u64> {
    queue
        .jobs()
        .iter()
        .filter(|job| job.state() == State::Active)
        .map(|job| job.id().0)
        .collect()
}

fn starts(actions: &[Action]) -> Vec<u64> {
    actions
        .iter()
        .filter_map(|action| match action {
            Action::Start(id) => Some(id.0),
            _ => None,
        })
        .collect()
}

#[test]
fn a_new_queue_has_three_slots_and_starts_ids_at_1() {
    let mut queue = Queue::new();
    assert_eq!(queue.slots(), 3);
    assert_eq!(add(&mut queue, "a", Priority::Normal), JobId(1));
    assert_eq!(add(&mut queue, "b", Priority::Normal), JobId(2));
    assert_eq!(Queue::default(), Queue::new());
}

#[test]
fn three_run_at_once_and_the_rest_wait_in_order() {
    let queue = queue_of(5);
    assert_eq!(running(&queue), [1, 2, 3]);
    assert_eq!(state(&queue, 4), State::Waiting);
    assert_eq!(state(&queue, 5), State::Waiting);
}

#[test]
fn an_id_is_never_used_twice_even_after_its_job_left_the_history() {
    let mut queue = Queue::new();
    let mut seen = Vec::new();
    for at in 0..150 {
        let id = add(&mut queue, &format!("f{at}"), Priority::Normal);
        assert!(!seen.contains(&id), "{id} again");
        seen.push(id);
        queue.cancel(id).expect("cancel");
    }
    assert_eq!(queue.history().len(), 100);
    assert_eq!(add(&mut queue, "next", Priority::Normal), JobId(151));
}

#[test]
fn debug_shows_no_path_and_no_ticket() {
    use crate::ticket::{Body, Ticket};
    let body = Body::Live {
        stream_id: [7; 16],
        sender_key: [0x55; 32],
    };
    let ticket = Ticket::new([1; 32], vec![], None, None, body).expect("valid");
    let text = ticket.encode();
    let mut queue = Queue::new();
    let work = Work::Receive {
        ticket,
        path: PathBuf::from("secret-name.iso"),
    };
    queue.add(work, Priority::Normal).expect("add");
    add(&mut queue, "other-secret.txt", Priority::Low);
    let printed = format!("{queue:?} {:?}", queue.jobs());
    for hidden in ["secret-name", "other-secret", &text, &text[7..40]] {
        assert!(!printed.contains(hidden), "{hidden} in {printed}");
    }
    assert!(printed.contains("Receive(<redacted>)"), "{printed}");
}

#[test]
fn the_history_keeps_the_last_100_newest_first_and_drops_the_oldest() {
    let mut queue = Queue::new();
    for at in 1..=105 {
        let id = add(&mut queue, &format!("f{at}"), Priority::Normal);
        match at % 3 {
            0 => drop(queue.finished(id).expect("finished")),
            1 => drop(queue.cancel(id).expect("cancel")),
            _ => drop(
                queue
                    .failed(id, crate::ErrorKind::Verify, 0)
                    .expect("failed"),
            ),
        }
    }
    let ids: Vec<u64> = queue.history().iter().map(|job| job.id().0).collect();
    assert_eq!(ids, (6..=105).rev().collect::<Vec<_>>());
    assert!(queue.job(JobId(5)).is_none(), "the oldest went");
    assert_eq!(state(&queue, 105), State::Done);
    assert_eq!(state(&queue, 104), State::Failed(crate::ErrorKind::Verify));
    assert_eq!(state(&queue, 103), State::Cancelled);
}

#[test]
fn a_finished_job_frees_its_slot_for_the_next_in_line() {
    let mut queue = queue_of(4);
    let actions = queue.finished(JobId(2)).expect("finished");
    assert_eq!(actions, [Action::Start(JobId(4)), Action::Save]);
    assert_eq!(running(&queue), [1, 3, 4]);
}

#[test]
fn pausing_an_active_job_stops_it_and_frees_its_slot() {
    let mut queue = queue_of(4);
    let actions = queue.pause(JobId(1)).expect("pause");
    assert_eq!(
        actions,
        [
            Action::Stop(JobId(1)),
            Action::Start(JobId(4)),
            Action::Save
        ]
    );
    assert_eq!(state(&queue, 1), State::Paused);
}

#[test]
fn pausing_a_waiting_job_keeps_it_from_starting() {
    let mut queue = queue_of(5);
    assert_eq!(queue.pause(JobId(4)).expect("pause"), [Action::Save]);
    let actions = queue.finished(JobId(1)).expect("finished");
    assert_eq!(starts(&actions), [5]);
    assert_eq!(state(&queue, 4), State::Paused);
}

#[test]
fn a_resumed_job_goes_back_to_its_place_and_not_to_the_end() {
    let mut queue = queue_of(6);
    queue.set_slots(1).expect("slots");
    queue.finished(JobId(1)).expect("finished");
    queue.finished(JobId(2)).expect("finished");
    assert_eq!(running(&queue), [3]);
    queue.pause(JobId(4)).expect("pause");
    queue.resume(JobId(4)).expect("resume");
    // Job 4 was added before 5 and 6, so it is still next.
    let actions = queue.finished(JobId(3)).expect("finished");
    assert_eq!(starts(&actions), [4]);
}

#[test]
fn cancelling_stops_an_active_job_and_puts_any_job_in_the_history() {
    let mut queue = queue_of(5);
    let actions = queue.cancel(JobId(2)).expect("cancel");
    assert_eq!(
        actions,
        [
            Action::Stop(JobId(2)),
            Action::Start(JobId(4)),
            Action::Save
        ]
    );
    assert_eq!(queue.cancel(JobId(5)).expect("cancel"), [Action::Save]);
    queue.pause(JobId(1)).expect("pause");
    queue.cancel(JobId(1)).expect("cancel a paused job");
    let ids: Vec<u64> = queue.history().iter().map(|job| job.id().0).collect();
    assert_eq!(ids, [1, 5, 2]);
    assert!(
        queue
            .history()
            .iter()
            .all(|job| job.state() == State::Cancelled)
    );
}

#[test]
fn lowering_the_slots_stops_nothing_and_starts_nothing_until_under_the_new_number() {
    let mut queue = queue_of(6);
    assert_eq!(queue.set_slots(1).expect("slots"), [Action::Save]);
    assert_eq!(running(&queue), [1, 2, 3]);
    let actions = queue.finished(JobId(1)).expect("finished");
    assert!(starts(&actions).is_empty(), "{actions:?}");
    queue.finished(JobId(2)).expect("finished");
    assert_eq!(running(&queue), [3]);
    let actions = queue.finished(JobId(3)).expect("finished");
    assert_eq!(starts(&actions), [4]);
}

#[test]
fn raising_the_slots_starts_the_next_in_line_and_the_range_is_1_to_64() {
    let mut queue = queue_of(6);
    let actions = queue.set_slots(5).expect("slots");
    assert_eq!(
        actions,
        [
            Action::Start(JobId(4)),
            Action::Start(JobId(5)),
            Action::Save
        ]
    );
    assert!(queue.set_slots(1).is_ok() && queue.set_slots(64).is_ok());
    assert_eq!(queue.set_slots(64).expect("same"), [], "no change, no save");
}
