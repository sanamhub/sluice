//! Strict priority, first in first out within a line, and no preemption.

use super::super::{Action, JobId, Priority, Queue, State};
use super::{add, queue_of, running, starts, state};

#[test]
fn a_high_job_added_while_three_normal_ones_run_waits_and_then_starts_first() {
    let mut queue = queue_of(4);
    let (high, actions) = queue
        .add(super::send("urgent"), Priority::High)
        .expect("add");
    // Nothing that runs is stopped to make room.
    assert_eq!(actions, [Action::Save]);
    assert_eq!(state(&queue, high.0), State::Waiting);
    let actions = queue.finished(JobId(1)).expect("finished");
    assert_eq!(starts(&actions), [high.0], "ahead of job 4, added earlier");
    assert_eq!(state(&queue, 4), State::Waiting);
}

#[test]
fn high_goes_before_normal_and_normal_before_low_and_first_in_first_out_within_each() {
    let mut queue = Queue::new();
    queue.set_slots(1).expect("slots");
    let first = add(&mut queue, "first", Priority::Low);
    let mut added = Vec::new();
    for (name, priority) in [
        ("low-a", Priority::Low),
        ("normal-a", Priority::Normal),
        ("high-a", Priority::High),
        ("normal-b", Priority::Normal),
        ("high-b", Priority::High),
        ("low-b", Priority::Low),
    ] {
        added.push((add(&mut queue, name, priority), name));
    }
    let mut order = Vec::new();
    let mut current = first;
    loop {
        let actions = queue.finished(current).expect("finished");
        let Some(&id) = starts(&actions).first() else {
            break;
        };
        let name = added.iter().find(|(at, _)| at.0 == id).expect("added").1;
        order.push(name);
        current = JobId(id);
    }
    assert_eq!(
        order,
        ["high-a", "high-b", "normal-a", "normal-b", "low-a", "low-b"]
    );
}

#[test]
fn a_low_job_waits_while_higher_ones_keep_coming_until_its_priority_is_raised() {
    let mut queue = Queue::new();
    queue.set_slots(1).expect("slots");
    let mut current = add(&mut queue, "running", Priority::Normal);
    let low = add(&mut queue, "low", Priority::Low);
    for round in 0..20 {
        add(&mut queue, &format!("n{round}"), Priority::Normal);
        let actions = queue.finished(current).expect("finished");
        let started = JobId(starts(&actions)[0]);
        assert_ne!(started, low, "round {round}");
        current = started;
    }
    assert_eq!(
        queue.set_priority(low, Priority::High).expect("raise"),
        [Action::Save]
    );
    add(&mut queue, "another", Priority::Normal);
    let actions = queue.finished(current).expect("finished");
    assert_eq!(starts(&actions), [low.0]);
}

#[test]
fn setting_a_priority_moves_a_job_to_another_line_and_keeps_its_order() {
    let mut queue = Queue::new();
    queue.set_slots(1).expect("slots");
    let running_job = add(&mut queue, "running", Priority::Normal);
    let early_low = add(&mut queue, "early", Priority::Low);
    let high = add(&mut queue, "high", Priority::High);
    // Lowered from high, it goes after `early_low`, which was added before it.
    let lowered = add(&mut queue, "lowered", Priority::High);
    queue.set_priority(lowered, Priority::Low).expect("lower");
    // Raised from low, it goes ahead of `high`, because it was added first.
    queue
        .set_priority(early_low, Priority::High)
        .expect("raise");
    let mut order = Vec::new();
    let mut current = running_job;
    while let Some(&id) = starts(&queue.finished(current).expect("finished")).first() {
        order.push(id);
        current = JobId(id);
    }
    assert_eq!(order, [early_low.0, high.0, lowered.0]);
}

#[test]
fn a_running_job_keeps_running_when_its_priority_changes() {
    let mut queue = queue_of(3);
    assert_eq!(
        queue.set_priority(JobId(2), Priority::Low).expect("lower"),
        [Action::Save]
    );
    assert_eq!(running(&queue), [1, 2, 3]);
    assert_eq!(
        queue.set_priority(JobId(2), Priority::Low).expect("same"),
        [],
        "no change, no save"
    );
}
