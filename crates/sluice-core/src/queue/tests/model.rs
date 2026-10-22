//! A second queue, written from the rules of the module documentation and not from the queue's
//! code, which the proptest runs beside the real one. It keeps its jobs in the order they were
//! added and picks the next to start with its own ranking, so a bug in `Job::place` or
//! `Priority::rank` shows up as a disagreement.

use super::super::{Action, JobId, Priority, Queue, State};
use super::props::Op;
use crate::ErrorKind;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Job {
    id: u64,
    priority: Priority,
    state: State,
    failures: u32,
}

/// What a restart keeps of a job: everything but the deadline of a retry.
type Kept = (u64, Priority, Option<State>, u32);

pub(super) struct Model {
    pub(super) now: u64,
    next_id: u64,
    slots: usize,
    /// Every job that is not history, in the order it was added.
    live: Vec<Job>,
    /// Newest first.
    history: Vec<Job>,
}

impl Model {
    pub(super) fn new() -> Self {
        Self {
            now: 0,
            next_id: 1,
            slots: 3,
            live: Vec::new(),
            history: Vec::new(),
        }
    }

    /// The actions the queue must return for `op`, or `None` if it must refuse it.
    pub(super) fn apply(&mut self, op: &Op) -> Option<Vec<Action>> {
        let kept = self.kept();
        // A tick is the timer going off, so the driver has none when it calls one.
        let timer = if matches!(op, Op::Tick(_)) {
            None
        } else {
            self.earliest_retry()
        };
        let stops = self.change(op)?;
        let mut actions: Vec<Action> = stops
            .into_iter()
            .map(|id| Action::Stop(JobId(id)))
            .collect();
        while self.count(State::Active) < self.slots {
            let Some(at) = self.next_to_start() else {
                break;
            };
            self.live[at].state = State::Active;
            actions.push(Action::Start(JobId(self.live[at].id)));
        }
        if self.kept() != kept {
            actions.push(Action::Save);
        }
        let after = self.earliest_retry();
        if after != timer {
            actions.push(Action::Wake { at_ms: after });
        }
        Some(actions)
    }

    /// Makes the change `op` asks for and returns the jobs it stops, or `None` if it is refused.
    fn change(&mut self, op: &Op) -> Option<Vec<u64>> {
        match *op {
            // The strategy's paths are valid and a run adds fewer than 1000 jobs, so an add is
            // never refused.
            Op::Add(priority, _) => {
                self.live.push(Job {
                    id: self.next_id,
                    priority,
                    state: State::Waiting,
                    failures: 0,
                });
                self.next_id += 1;
                Some(vec![])
            }
            Op::Pause(id) => self.pause(id),
            Op::Resume(id) => {
                let job = self.live_job(id)?;
                if job.state != State::Paused {
                    return None;
                }
                job.state = State::Waiting;
                Some(vec![])
            }
            Op::Cancel(id) => {
                let stops = self.stops(id)?;
                self.retire(id, State::Cancelled);
                Some(stops)
            }
            Op::SetPriority(id, priority) => {
                self.live_job(id)?.priority = priority;
                Some(vec![])
            }
            Op::Slots(slots) => {
                if !(1..=64).contains(&slots) {
                    return None;
                }
                self.slots = usize::from(slots);
                Some(vec![])
            }
            Op::Progress(id) => {
                self.active_job(id)?.failures = 0;
                Some(vec![])
            }
            Op::Finished(id) => {
                self.active_job(id)?;
                self.retire(id, State::Done);
                Some(vec![])
            }
            Op::Failed(id, kind) => self.fail(id, kind).map(|()| vec![]),
            Op::Tick(by) => {
                self.tick(by);
                Some(vec![])
            }
        }
    }

    fn tick(&mut self, by: u64) {
        self.now += by;
        let now = self.now;
        for job in &mut self.live {
            if matches!(job.state, State::Retrying { at_ms } if at_ms <= now) {
                job.state = State::Waiting;
            }
        }
    }

    /// The job `id` if it runs, as a list of the jobs to stop, or `None` if it is not live.
    fn stops(&mut self, id: u64) -> Option<Vec<u64>> {
        let active = self.live_job(id)?.state == State::Active;
        Some(if active { vec![id] } else { vec![] })
    }

    fn pause(&mut self, id: u64) -> Option<Vec<u64>> {
        let stops = self.stops(id)?;
        let job = self.live_job(id)?;
        if job.state == State::Paused {
            return None;
        }
        job.state = State::Paused;
        Some(stops)
    }

    fn fail(&mut self, id: u64, kind: ErrorKind) -> Option<()> {
        let now = self.now;
        let job = self.active_job(id)?;
        if kind != ErrorKind::Network {
            self.retire(id, State::Failed(kind));
            return Some(());
        }
        job.failures += 1;
        if job.failures == 15 {
            self.retire(id, State::Failed(ErrorKind::Network));
        } else {
            let wait = (1..job.failures).fold(1000u64, |wait, _| (wait * 2).min(300_000));
            job.state = State::Retrying {
                at_ms: now.saturating_add(wait),
            };
        }
        Some(())
    }

    fn live_job(&mut self, id: u64) -> Option<&mut Job> {
        self.live.iter_mut().find(|job| job.id == id)
    }

    fn active_job(&mut self, id: u64) -> Option<&mut Job> {
        self.live_job(id).filter(|job| job.state == State::Active)
    }

    fn retire(&mut self, id: u64, state: State) {
        let at = self.live.iter().position(|job| job.id == id).expect("live");
        let mut job = self.live.remove(at);
        job.state = state;
        self.history.insert(0, job);
        self.history.truncate(100);
    }

    fn count(&self, state: State) -> usize {
        self.live.iter().filter(|job| job.state == state).count()
    }

    /// The waiting job of the highest line that was added first, by its index in `live`.
    fn next_to_start(&self) -> Option<usize> {
        let line = |priority| match priority {
            Priority::High => 0,
            Priority::Normal => 1,
            Priority::Low => 2,
        };
        (0..self.live.len())
            .filter(|&at| self.live[at].state == State::Waiting)
            .min_by_key(|&at| (line(self.live[at].priority), at))
    }

    pub(super) fn earliest_retry(&self) -> Option<u64> {
        self.live
            .iter()
            .filter_map(|job| match job.state {
                State::Retrying { at_ms } => Some(at_ms),
                _ => None,
            })
            .min()
    }

    fn kept(&self) -> (u64, usize, Vec<Kept>, Vec<Kept>) {
        let keep = |job: &Job| {
            let state = (!matches!(job.state, State::Retrying { .. })).then_some(job.state);
            (job.id, job.priority, state, job.failures)
        };
        (
            self.next_id,
            self.slots,
            self.live.iter().map(keep).collect(),
            self.history.iter().map(keep).collect(),
        )
    }

    /// Checks that `queue` holds what the model holds: the same jobs in the same states, with the
    /// same counts of failures, and the same history, newest first.
    pub(super) fn check(&self, queue: &Queue) {
        let seen = |jobs: Vec<&super::super::Job>| -> Vec<Job> {
            jobs.into_iter()
                .map(|job| Job {
                    id: job.id().0,
                    priority: job.priority(),
                    state: job.state(),
                    failures: job.network_failures(),
                })
                .collect()
        };
        assert_eq!(seen(queue.jobs()), self.live, "the jobs differ");
        assert_eq!(seen(queue.history()), self.history, "the history differs");
        assert_eq!(usize::from(queue.slots()), self.slots);
    }
}
