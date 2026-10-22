//! Random command sequences, checked against what the queue promises after every command.

use std::collections::BTreeSet;

use proptest::prelude::*;

use super::super::{Action, JobId, Priority, Queue, State, Work};
use crate::{Error, ErrorKind};

#[derive(Clone, Debug)]
enum Op {
    Add(Priority),
    Pause(u64),
    Resume(u64),
    Cancel(u64),
    SetPriority(u64, Priority),
    Slots(u8),
    Progress(u64),
    Finished(u64),
    Failed(u64, ErrorKind),
    /// Moves the clock on by this many milliseconds and ticks.
    Tick(u64),
}

fn priority() -> impl Strategy<Value = Priority> {
    prop_oneof![
        Just(Priority::High),
        Just(Priority::Normal),
        Just(Priority::Low)
    ]
}

fn kind() -> impl Strategy<Value = ErrorKind> {
    prop_oneof![
        4 => Just(ErrorKind::Network),
        1 => Just(ErrorKind::Verify),
        1 => Just(ErrorKind::Storage),
        1 => Just(ErrorKind::Policy),
        1 => Just(ErrorKind::Cancelled),
        1 => Just(ErrorKind::Bug),
    ]
}

/// Ids from 0, which is never a job, to well past the jobs a run adds.
fn op() -> impl Strategy<Value = Op> {
    let id = 0u64..40;
    prop_oneof![
        6 => priority().prop_map(Op::Add),
        2 => id.clone().prop_map(Op::Pause),
        2 => id.clone().prop_map(Op::Resume),
        1 => id.clone().prop_map(Op::Cancel),
        1 => (id.clone(), priority()).prop_map(|(i, p)| Op::SetPriority(i, p)),
        1 => (0u8..8).prop_map(Op::Slots),
        2 => id.clone().prop_map(Op::Progress),
        4 => id.clone().prop_map(Op::Finished),
        4 => (id, kind()).prop_map(|(i, k)| Op::Failed(i, k)),
        3 => (0u64..400_000).prop_map(Op::Tick),
    ]
}

/// What a driver of the queue would know, kept apart from the queue.
#[derive(Default)]
struct Model {
    now: u64,
    added: Vec<JobId>,
    /// Ids in the order they went to the history.
    retired: Vec<JobId>,
    /// The jobs the driver was told to start and not since told to stop, and not reported over.
    running: BTreeSet<JobId>,
}

fn apply(queue: &mut Queue, model: &mut Model, op: &Op) -> Result<Vec<Action>, Error> {
    match *op {
        Op::Add(priority) => {
            let path = format!("file-{}", model.added.len()).into();
            let (id, actions) = queue.add(Work::Send { path }, priority)?;
            assert!(model.added.iter().all(|old| *old < id), "{id} reused");
            model.added.push(id);
            Ok(actions)
        }
        Op::Pause(id) => queue.pause(JobId(id)),
        Op::Resume(id) => queue.resume(JobId(id)),
        Op::Cancel(id) => queue.cancel(JobId(id)),
        Op::SetPriority(id, priority) => queue.set_priority(JobId(id), priority),
        Op::Slots(slots) => queue.set_slots(slots),
        Op::Progress(id) => queue.progress(JobId(id)),
        Op::Finished(id) => over(model, JobId(id), queue.finished(JobId(id))),
        Op::Failed(id, kind) => over(model, JobId(id), queue.failed(JobId(id), kind, model.now)),
        Op::Tick(by) => {
            model.now += by;
            queue.tick(model.now)
        }
    }
}

/// A report that a job is over takes it off the driver's list of what runs.
fn over(
    model: &mut Model,
    id: JobId,
    result: Result<Vec<Action>, Error>,
) -> Result<Vec<Action>, Error> {
    if result.is_ok() {
        assert!(
            model.running.remove(&id),
            "{id} was reported over but never started"
        );
    }
    result
}

fn active(queue: &Queue) -> BTreeSet<JobId> {
    queue
        .jobs()
        .iter()
        .filter(|job| job.state() == State::Active)
        .map(|job| job.id())
        .collect()
}

/// The stops come before the starts, the starts before the save, and the timer last.
fn check_order(actions: &[Action]) {
    let rank = |action: &Action| match action {
        Action::Stop(_) => 0,
        Action::Start(_) => 1,
        Action::Save => 2,
        Action::Wake { .. } => 3,
    };
    assert!(
        actions.windows(2).all(|w| rank(&w[0]) <= rank(&w[1])),
        "{actions:?}"
    );
}

fn check_starts(queue: &Queue, model: &mut Model, actions: &[Action], before: usize) {
    for action in actions {
        match action {
            Action::Stop(id) => assert!(model.running.remove(id), "stopped {id}, not running"),
            Action::Start(id) => assert!(model.running.insert(*id), "started {id} twice"),
            _ => {}
        }
    }
    let slots = usize::from(queue.slots());
    let now = active(queue);
    assert_eq!(
        now, model.running,
        "the queue and the driver disagree on what runs"
    );
    // Lowering the slots stops nothing, so more can run than there are slots, but no start
    // ever adds to that.
    assert!(
        now.len() <= before.max(slots),
        "{} run with {slots} slots",
        now.len()
    );
    let started: Vec<&crate::queue::Job> = actions
        .iter()
        .filter_map(|a| match a {
            Action::Start(id) => queue.job(*id),
            _ => None,
        })
        .collect();
    if !started.is_empty() {
        assert!(now.len() <= slots, "a start past the slots");
    }
    let waiting: Vec<_> = queue
        .jobs()
        .into_iter()
        .filter(|job| job.state() == State::Waiting)
        .collect();
    if now.len() < slots {
        assert!(waiting.is_empty(), "a free slot and a job waiting for it");
    }
    assert!(
        started.windows(2).all(|w| w[0].place() < w[1].place()),
        "starts out of order"
    );
    for job in &started {
        for left in &waiting {
            assert!(
                job.place() < left.place(),
                "{} started ahead of {}",
                job.id(),
                left.id()
            );
        }
    }
}

/// Every job is in exactly one place, and every job ever added is still there unless it was
/// one of the oldest in a full history.
fn check_jobs(queue: &Queue, model: &mut Model, history_before: &[JobId]) {
    let live: Vec<JobId> = queue.jobs().iter().map(|job| job.id()).collect();
    let history: Vec<JobId> = queue.history().iter().map(|job| job.id()).collect();
    assert!(queue.jobs().iter().all(|job| !job.state().is_history()));
    assert!(queue.history().iter().all(|job| job.state().is_history()));
    assert!(history.len() <= 100);
    for id in history.iter().filter(|id| !history_before.contains(id)) {
        model.retired.push(*id);
    }
    let kept: Vec<JobId> = model.retired.iter().rev().take(100).copied().collect();
    assert_eq!(
        history.iter().copied().collect::<BTreeSet<_>>(),
        kept.iter().copied().collect()
    );
    let retired: BTreeSet<JobId> = model.retired.iter().copied().collect();
    let expected: Vec<JobId> = model
        .added
        .iter()
        .filter(|id| !retired.contains(id))
        .copied()
        .collect();
    assert_eq!(live, expected, "a job was lost or appeared");
    let all: BTreeSet<JobId> = live.iter().chain(&history).copied().collect();
    assert_eq!(all.len(), live.len() + history.len(), "a job in two places");
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn every_command_sequence_keeps_the_promises(ops in prop::collection::vec(op(), 1..300)) {
        let mut queue = Queue::new();
        let mut model = Model::default();
        for op in &ops {
            let before = queue.clone();
            let running_before = active(&queue).len();
            let history_before: Vec<JobId> = queue.history().iter().map(|job| job.id()).collect();
            match apply(&mut queue, &mut model, op) {
                Ok(actions) => {
                    check_order(&actions);
                    check_starts(&queue, &mut model, &actions, running_before);
                    check_jobs(&queue, &mut model, &history_before);
                }
                Err(error) => {
                    prop_assert!(matches!(error, Error::Policy(_)), "{op:?}: {error}");
                    prop_assert_eq!(&queue, &before, "{:?} failed and changed the queue", op);
                }
            }
        }
    }
}
