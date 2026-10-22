//! The commands of the queue. Each checks its arguments first and changes nothing when it fails,
//! then returns the actions its change asks for.

use super::{
    Action, HISTORY, Job, JobId, MAX_JOBS, MAX_NETWORK_FAILURES, MAX_SLOTS, Priority, Queue, State,
    Work, backoff_ms, check_path, refuse,
};
use crate::{Error, ErrorKind};

impl Queue {
    /// Adds a job at the end of its line and returns its id.
    ///
    /// Fails with [`Error::Policy`] if the path is empty or longer than 4096 bytes (UTF-16 units
    /// on Windows), or if 1000 jobs are already in the queue, not counting history.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::queue::{Action, JobId, Priority, Queue, Work};
    ///
    /// let mut queue = Queue::new();
    /// let (id, actions) = queue.add(Work::Send { path: "a".into() }, Priority::Normal)?;
    /// assert_eq!((id, actions), (JobId(1), vec![Action::Start(JobId(1)), Action::Save]));
    /// assert!(queue.add(Work::Send { path: "".into() }, Priority::Normal).is_err());
    /// # Ok::<(), sluice_core::Error>(())
    /// ```
    pub fn add(&mut self, work: Work, priority: Priority) -> Result<(JobId, Vec<Action>), Error> {
        check_path(&work)?;
        if self.jobs.len() >= MAX_JOBS {
            return Err(Error::Policy(format!(
                "the queue holds {MAX_JOBS} jobs, which is as many as it takes. Wait for some to \
                 finish, or cancel some"
            )));
        }
        let id = JobId(self.next_id);
        let next = self
            .next_id
            .checked_add(1)
            .ok_or_else(|| Error::Policy("the queue has used every job id there is".to_string()))?;
        self.next_id = next;
        let job = Job {
            id,
            work,
            priority,
            state: State::Waiting,
            failures: 0,
        };
        self.jobs.insert(id, job);
        Ok((id, self.settle(Vec::new(), true)))
    }

    /// Keeps a job from running. An active job is stopped and its slot goes to the next in line.
    /// A job that waits, or waits to retry, keeps its place and does not start.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::queue::{Action, Priority, Queue, Work};
    ///
    /// let mut queue = Queue::new();
    /// let (id, _) = queue.add(Work::Send { path: "a".into() }, Priority::Normal)?;
    /// assert_eq!(queue.pause(id)?, [Action::Stop(id), Action::Save]);
    /// assert!(queue.pause(id).is_err(), "already paused");
    /// # Ok::<(), sluice_core::Error>(())
    /// ```
    pub fn pause(&mut self, id: JobId) -> Result<Vec<Action>, Error> {
        let job = self.live(id)?;
        let stop = match job.state {
            State::Active => true,
            State::Waiting | State::Retrying { .. } => false,
            _ => return Err(refuse(id, "is already paused")),
        };
        job.state = State::Paused;
        let actions = if stop { vec![Action::Stop(id)] } else { vec![] };
        Ok(self.settle(actions, true))
    }

    /// Puts a paused job back in its place in line: by the order it was added in, not at the end.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::queue::{Action, Priority, Queue, Work};
    ///
    /// let mut queue = Queue::new();
    /// let (id, _) = queue.add(Work::Send { path: "a".into() }, Priority::Normal)?;
    /// queue.pause(id)?;
    /// assert_eq!(queue.resume(id)?, [Action::Start(id), Action::Save]);
    /// assert!(queue.resume(id).is_err(), "not paused");
    /// # Ok::<(), sluice_core::Error>(())
    /// ```
    pub fn resume(&mut self, id: JobId) -> Result<Vec<Action>, Error> {
        let job = self.live(id)?;
        if job.state != State::Paused {
            return Err(refuse(id, "is not paused, so it cannot be resumed"));
        }
        job.state = State::Waiting;
        Ok(self.settle(Vec::new(), true))
    }

    /// Cancels a job that is not history, stopping it if it runs. It goes to the history.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::queue::{Action, Priority, Queue, Work};
    ///
    /// let mut queue = Queue::new();
    /// let (id, _) = queue.add(Work::Send { path: "a".into() }, Priority::Normal)?;
    /// assert_eq!(queue.cancel(id)?, [Action::Stop(id), Action::Save]);
    /// assert!(queue.cancel(id).is_err(), "already cancelled");
    /// # Ok::<(), sluice_core::Error>(())
    /// ```
    pub fn cancel(&mut self, id: JobId) -> Result<Vec<Action>, Error> {
        let stop = self.live(id)?.state == State::Active;
        self.retire(id, State::Cancelled);
        let actions = if stop { vec![Action::Stop(id)] } else { vec![] };
        Ok(self.settle(actions, true))
    }

    /// Moves a job that is not history to another line. It keeps its order, so among the jobs of
    /// its new line it goes where its id puts it. A job that runs keeps running.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::queue::{Action, Priority, Queue, Work};
    ///
    /// let mut queue = Queue::new();
    /// queue.set_slots(1)?;
    /// queue.add(Work::Send { path: "a".into() }, Priority::Normal)?;
    /// let (low, _) = queue.add(Work::Send { path: "b".into() }, Priority::Low)?;
    /// assert_eq!(queue.set_priority(low, Priority::High)?, [Action::Save]);
    /// # Ok::<(), sluice_core::Error>(())
    /// ```
    pub fn set_priority(&mut self, id: JobId, priority: Priority) -> Result<Vec<Action>, Error> {
        let job = self.live(id)?;
        let changed = job.priority != priority;
        job.priority = priority;
        Ok(self.settle(Vec::new(), changed))
    }

    /// Sets how many jobs run at once, from 1 to 64. Raising it starts the next jobs in line.
    /// Lowering it stops nothing: fewer jobs start until fewer than `slots` run.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::queue::Queue;
    ///
    /// let mut queue = Queue::new();
    /// assert!(queue.set_slots(0).is_err() && queue.set_slots(65).is_err());
    /// assert!(queue.set_slots(64).is_ok());
    /// ```
    pub fn set_slots(&mut self, slots: u8) -> Result<Vec<Action>, Error> {
        if !(1..=MAX_SLOTS).contains(&slots) {
            return Err(Error::Policy(format!(
                "{slots} transfers at once is out of range: it is from 1 to {MAX_SLOTS}"
            )));
        }
        let changed = self.slots != slots;
        self.slots = slots;
        Ok(self.settle(Vec::new(), changed))
    }

    /// Says an active job moved verified bytes. Its count of network failures in a row starts
    /// again from zero.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::ErrorKind;
    /// use sluice_core::queue::{Priority, Queue, Work};
    ///
    /// let mut queue = Queue::new();
    /// let (id, _) = queue.add(Work::Send { path: "a".into() }, Priority::Normal)?;
    /// queue.failed(id, ErrorKind::Network, 0)?;
    /// queue.tick(1000)?;
    /// queue.progress(id)?;
    /// assert_eq!(queue.job(id).unwrap().network_failures(), 0);
    /// # Ok::<(), sluice_core::Error>(())
    /// ```
    pub fn progress(&mut self, id: JobId) -> Result<Vec<Action>, Error> {
        let job = self.active(id, "made progress")?;
        let changed = job.failures != 0;
        job.failures = 0;
        Ok(self.settle(Vec::new(), changed))
    }

    /// Says an active job finished. It goes to the history and its slot to the next in line.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::queue::{Priority, Queue, State, Work};
    ///
    /// let mut queue = Queue::new();
    /// let (id, _) = queue.add(Work::Send { path: "a".into() }, Priority::Normal)?;
    /// queue.finished(id)?;
    /// assert_eq!(queue.job(id).unwrap().state(), State::Done);
    /// assert!(queue.finished(id).is_err(), "it is history now");
    /// # Ok::<(), sluice_core::Error>(())
    /// ```
    pub fn finished(&mut self, id: JobId) -> Result<Vec<Action>, Error> {
        self.active(id, "finished")?;
        self.retire(id, State::Done);
        Ok(self.settle(Vec::new(), true))
    }

    /// Says an active job failed with an error of `kind`, at `now_ms` on the caller's monotonic
    /// clock. A network failure waits to retry, unless it is the 10th in a row, which fails the
    /// job. Any other kind fails it at once.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::ErrorKind;
    /// use sluice_core::queue::{Action, Priority, Queue, State, Work};
    ///
    /// let mut queue = Queue::new();
    /// let (id, _) = queue.add(Work::Send { path: "a".into() }, Priority::Normal)?;
    /// let actions = queue.failed(id, ErrorKind::Network, 5000)?;
    /// assert_eq!(actions, [Action::Save, Action::Wake { at_ms: 6000 }]);
    /// assert_eq!(queue.job(id).unwrap().state(), State::Retrying { at_ms: 6000 });
    /// # Ok::<(), sluice_core::Error>(())
    /// ```
    pub fn failed(
        &mut self,
        id: JobId,
        kind: ErrorKind,
        now_ms: u64,
    ) -> Result<Vec<Action>, Error> {
        let job = self.active(id, "failed")?;
        if kind != ErrorKind::Network {
            self.retire(id, State::Failed(kind));
        } else if job.failures + 1 >= MAX_NETWORK_FAILURES {
            job.failures = MAX_NETWORK_FAILURES;
            self.retire(id, State::Failed(kind));
        } else {
            job.failures += 1;
            let at_ms = now_ms.saturating_add(backoff_ms(job.failures));
            job.state = State::Retrying { at_ms };
        }
        let mut actions = self.settle(Vec::new(), true);
        actions.extend(self.wake());
        Ok(actions)
    }

    /// The timer the queue asked for has fired, or any other time has come. Every job whose wait
    /// to retry is over at `now_ms` is in line again, in its old place.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::ErrorKind;
    /// use sluice_core::queue::{Action, Priority, Queue, Work};
    ///
    /// let mut queue = Queue::new();
    /// let (id, _) = queue.add(Work::Send { path: "a".into() }, Priority::Normal)?;
    /// queue.failed(id, ErrorKind::Network, 0)?;
    /// assert_eq!(queue.tick(999)?, [Action::Wake { at_ms: 1000 }]);
    /// assert_eq!(queue.tick(1000)?, [Action::Start(id), Action::Save]);
    /// # Ok::<(), sluice_core::Error>(())
    /// ```
    pub fn tick(&mut self, now_ms: u64) -> Result<Vec<Action>, Error> {
        let mut changed = false;
        for job in self.jobs.values_mut() {
            if let State::Retrying { at_ms } = job.state
                && at_ms <= now_ms
            {
                job.state = State::Waiting;
                changed = true;
            }
        }
        let mut actions = self.settle(Vec::new(), changed);
        actions.extend(self.wake());
        Ok(actions)
    }

    /// A job that is not history, or the error for an id that is not one.
    fn live(&mut self, id: JobId) -> Result<&mut Job, Error> {
        if let Some(old) = self.history.iter().find(|job| job.id == id) {
            let over = match old.state {
                State::Done => "has finished",
                State::Cancelled => "was cancelled",
                _ => "has failed",
            };
            return Err(refuse(id, over));
        }
        self.jobs
            .get_mut(&id)
            .ok_or_else(|| Error::Policy(format!("there is no job {id}")))
    }

    /// An active job, or the error for one that is not, for a report that it `did` something.
    fn active(&mut self, id: JobId, did: &str) -> Result<&mut Job, Error> {
        let job = self.live(id)?;
        if job.state != State::Active {
            return Err(refuse(
                id,
                &format!("is not running, so it cannot have {did}"),
            ));
        }
        Ok(job)
    }

    /// Moves a job that is not history to the front of the history, in `state`, and drops the
    /// oldest beyond 100.
    fn retire(&mut self, id: JobId, state: State) {
        if let Some(mut job) = self.jobs.remove(&id) {
            job.state = state;
            self.history.push_front(job);
            self.history.truncate(HISTORY);
        }
    }

    /// Fills the free slots and finishes the list of actions: `actions` are the stops, then the
    /// starts follow, then a save if `changed` or anything started.
    fn settle(&mut self, mut actions: Vec<Action>, changed: bool) -> Vec<Action> {
        let mut started = false;
        while self.running() < usize::from(self.slots) {
            let Some(id) = self.next_in_line() else { break };
            if let Some(job) = self.jobs.get_mut(&id) {
                job.state = State::Active;
            }
            actions.push(Action::Start(id));
            started = true;
        }
        if changed || started {
            actions.push(Action::Save);
        }
        actions
    }

    fn running(&self) -> usize {
        self.jobs
            .values()
            .filter(|job| job.state == State::Active)
            .count()
    }

    /// The waiting job that starts next: the first of the highest line.
    fn next_in_line(&self) -> Option<JobId> {
        self.jobs
            .values()
            .filter(|job| job.state == State::Waiting)
            .min_by_key(|job| job.place())
            .map(|job| job.id)
    }

    /// The timer for the earliest retry, if a job waits for one.
    fn wake(&self) -> Option<Action> {
        self.jobs
            .values()
            .filter_map(|job| match job.state {
                State::Retrying { at_ms } => Some(at_ms),
                _ => None,
            })
            .min()
            .map(|at_ms| Action::Wake { at_ms })
    }
}
