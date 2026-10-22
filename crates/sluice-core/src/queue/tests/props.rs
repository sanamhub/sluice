//! Random command sequences, run on the queue and on a model of it, which must agree after every
//! command.

use std::collections::BTreeSet;
use std::path::PathBuf;

use proptest::prelude::*;

use super::super::{Action, JobId, Priority, Queue, State, Work};
use super::model::Model;
use crate::ticket::{Body, Format, Ticket};
use crate::{Error, ErrorKind};

/// One command, with ids that may name no job. The save tests build queues from these too.
#[derive(Clone, Debug)]
pub(super) enum Op {
    Add(Priority, Work),
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

/// A path with letters outside ASCII, and on Unix bytes that are not UTF-8 at all.
fn path() -> BoxedStrategy<PathBuf> {
    let text = "[a-z0-9 ._\u{e9}\u{4e2d}\u{1f600}-]{1,16}".prop_map(PathBuf::from);
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        let raw = prop::collection::vec(1u8.., 1..16)
            .prop_map(|bytes| PathBuf::from(std::ffi::OsString::from_vec(bytes)));
        prop_oneof![3 => text, 1 => raw].boxed()
    }
    #[cfg(not(unix))]
    text.boxed()
}

fn work() -> impl Strategy<Value = Work> {
    let ticket = (
        any::<[u8; 32]>(),
        any::<u64>(),
        prop::option::of(any::<u64>()),
    )
        .prop_map(|(root, size, expiry)| {
            let body = Body::Static {
                root,
                size,
                format: Format::Raw,
            };
            Ticket::new([1; 32], vec![], None, expiry, body).expect("a valid ticket")
        });
    prop_oneof![
        path().prop_map(|path| Work::Send { path }),
        (ticket, path()).prop_map(|(ticket, path)| Work::Receive { ticket, path }),
    ]
}

/// Ids from 0, which is never a job, to well past the jobs a run adds.
pub(super) fn op() -> impl Strategy<Value = Op> {
    let id = 0u64..40;
    prop_oneof![
        6 => (priority(), work()).prop_map(|(p, w)| Op::Add(p, w)),
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

/// Applies `op` to the queue at `now`, the clock before the command.
fn apply(queue: &mut Queue, now: u64, op: &Op) -> Result<Vec<Action>, Error> {
    match *op {
        Op::Add(priority, ref work) => queue.add(work.clone(), priority).map(|(_, a)| a),
        Op::Pause(id) => queue.pause(JobId(id)),
        Op::Resume(id) => queue.resume(JobId(id)),
        Op::Cancel(id) => queue.cancel(JobId(id)),
        Op::SetPriority(id, priority) => queue.set_priority(JobId(id), priority),
        Op::Slots(slots) => queue.set_slots(slots),
        Op::Progress(id) => queue.progress(JobId(id)),
        Op::Finished(id) => queue.finished(JobId(id)),
        Op::Failed(id, kind) => queue.failed(JobId(id), kind, now),
        Op::Tick(by) => queue.tick(now + by),
    }
}

/// The queue that `ops` leave, with the commands that fail skipped.
pub(super) fn run(ops: &[Op]) -> Queue {
    let mut queue = Queue::new();
    let mut now = 0;
    for op in ops {
        let _ = apply(&mut queue, now, op);
        if let Op::Tick(by) = op {
            now += by;
        }
    }
    queue
}

/// What a driver of the queue knows from the actions alone.
#[derive(Default)]
struct Driver {
    /// The jobs it was told to start and not since told to stop or reported over.
    running: BTreeSet<JobId>,
    /// Its one timer.
    timer: Option<u64>,
}

impl Driver {
    fn follow(&mut self, op: &Op, actions: &[Action]) {
        match *op {
            Op::Finished(id) | Op::Failed(id, _) => {
                assert!(
                    self.running.remove(&JobId(id)),
                    "{id} over but never started"
                );
            }
            Op::Tick(_) => self.timer = None,
            _ => {}
        }
        for action in actions {
            match *action {
                Action::Stop(id) => assert!(self.running.remove(&id), "stopped {id}, not running"),
                Action::Start(id) => assert!(self.running.insert(id), "started {id} twice"),
                Action::Save => {}
                Action::Wake { at_ms } => self.timer = at_ms,
            }
        }
    }
}

/// The stops come before the starts, the starts before the save, and the timer last. There is at
/// most one save and one timer.
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
    for last in [2, 3] {
        assert!(
            actions.iter().filter(|a| rank(a) == last).count() <= 1,
            "{actions:?}"
        );
    }
}

/// What the driver sees agrees with the queue: the same jobs run, no start goes past the slots,
/// a free slot never sits beside a waiting job, and the timer is set for the earliest retry.
fn check_driver(queue: &Queue, model: &Model, driver: &Driver, actions: &[Action]) {
    let active: BTreeSet<JobId> = queue
        .jobs()
        .iter()
        .filter(|job| job.state() == State::Active)
        .map(|job| job.id())
        .collect();
    assert_eq!(
        active, driver.running,
        "the queue and the driver disagree on what runs"
    );
    let slots = usize::from(queue.slots());
    if actions.iter().any(|a| matches!(a, Action::Start(_))) {
        assert!(active.len() <= slots, "a start past the slots");
    }
    if active.len() < slots {
        assert!(
            queue.jobs().iter().all(|job| job.state() != State::Waiting),
            "a free slot and a job waiting for it"
        );
    }
    assert_eq!(
        driver.timer,
        model.earliest_retry(),
        "the timer is not the earliest retry"
    );
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn every_command_sequence_does_what_the_model_does(ops in prop::collection::vec(op(), 1..300)) {
        let mut queue = Queue::new();
        let mut model = Model::new();
        let mut driver = Driver::default();
        for op in &ops {
            let before = queue.clone();
            let got = apply(&mut queue, model.now, op);
            let expected = model.apply(op);
            match (got, expected) {
                (Ok(actions), Some(expected)) => {
                    prop_assert_eq!(&actions, &expected, "{:?}", op);
                    check_order(&actions);
                    driver.follow(op, &actions);
                    check_driver(&queue, &model, &driver, &actions);
                }
                (Err(error), None) => {
                    prop_assert!(matches!(error, Error::Policy(_)), "{op:?}: {error}");
                    prop_assert_eq!(&queue, &before, "{:?} failed and changed the queue", op);
                }
                (got, expected) => {
                    prop_assert!(false, "{op:?}: the queue gave {got:?}, the model {expected:?}");
                }
            }
            model.check(&queue);
        }
    }
}
