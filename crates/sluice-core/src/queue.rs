//! The transfer queue of `docs/design.md`: which transfers run, which wait, and in what order.
//!
//! It is a model with no IO, as ADR 0008 asks of the protocol crates. A command goes in and the
//! actions to take come out: start a job, stop one, save the queue, set the timer. The queue holds
//! no clock, no thread and no socket. Time comes in from the caller as milliseconds on a monotonic
//! clock, and only the commands that need it take it.
//!
//! - Three jobs run at once by default, from 1 to 64.
//! - High always starts before normal, and normal before low. Within a line, first in, first out.
//!   Nothing that runs is stopped to make room.
//! - Only a network failure is retried, after 1 s, 2 s, 4 s and so on up to 5 minutes. The 15th
//!   network failure in a row fails the job, about 34 minutes after the first, and progress starts
//!   the count again. Every other kind of failure fails the job at once, because a retry cannot
//!   fix it.
//! - The queue asks for a timer only while a job waits to retry, and clears it when none does, so
//!   an idle driver has nothing to wake it.
//! - Done, failed and cancelled jobs stay in the history, newest first, up to the last 100.
//!
//! A command on an id the queue does not have, or one that makes no sense in the job's state, is
//! [`Error::Policy`] and changes nothing.

mod commands;
mod job;
mod save;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, VecDeque};

pub use job::{Action, Job, JobId, Priority, State, Work};

use crate::{Error, Home};

/// How many jobs run at once unless the user says otherwise.
const DEFAULT_SLOTS: u8 = 3;
/// The most jobs that can be set to run at once.
const MAX_SLOTS: u8 = 64;
/// How many finished, failed and cancelled jobs are kept.
const HISTORY: usize = 100;
/// The most jobs that are not history. It keeps the saved queue under the size a load accepts,
/// with the longest path and ticket in every job.
const MAX_JOBS: usize = 1000;
/// The longest path a job takes: bytes on Unix, UTF-16 units on Windows.
const MAX_PATH_UNITS: usize = 4096;
/// The network failure in a row that fails a job. With the waits below, a job retries for about 34
/// minutes before it fails, which outlasts a laptop moving between networks or a router restart.
/// A receive uses the same count and waits.
pub const MAX_NETWORK_FAILURES: u32 = 15;
/// The wait after the first network failure, doubled after each one that follows.
const FIRST_WAIT_MS: u64 = 1000;
/// The longest wait before a retry.
const MAX_WAIT_MS: u64 = 5 * 60 * 1000;

/// The transfer queue. See the [module documentation](self) for its rules.
///
/// # Examples
///
/// ```
/// use sluice_core::queue::{Action, Priority, Queue, State, Work};
///
/// let mut queue = Queue::new();
/// for name in ["a", "b", "c", "d"] {
///     queue.add(Work::Send { path: name.into() }, Priority::Normal)?;
/// }
/// // Three slots: the fourth job waits until one is free.
/// let fourth = queue.jobs()[3].id();
/// assert_eq!(queue.job(fourth).unwrap().state(), State::Waiting);
/// let first = queue.jobs()[0].id();
/// assert_eq!(queue.finished(first)?, [Action::Start(fourth), Action::Save]);
/// assert_eq!(queue.history()[0].state(), State::Done);
/// # Ok::<(), sluice_core::Error>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Queue {
    /// Every job that is not history, by id.
    jobs: BTreeMap<JobId, Job>,
    /// Done, failed and cancelled jobs, newest first.
    history: VecDeque<Job>,
    next_id: u64,
    slots: u8,
}

impl Default for Queue {
    fn default() -> Self {
        Self::new()
    }
}

impl Queue {
    /// An empty queue with three slots, whose first job gets id 1.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::queue::Queue;
    ///
    /// let queue = Queue::new();
    /// assert_eq!(queue.slots(), 3);
    /// assert!(queue.jobs().is_empty() && queue.history().is_empty());
    /// ```
    pub fn new() -> Self {
        Self {
            jobs: BTreeMap::new(),
            history: VecDeque::new(),
            next_id: 1,
            slots: DEFAULT_SLOTS,
        }
    }

    /// How many jobs may run at once.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::queue::Queue;
    ///
    /// let mut queue = Queue::new();
    /// queue.set_slots(8)?;
    /// assert_eq!(queue.slots(), 8);
    /// # Ok::<(), sluice_core::Error>(())
    /// ```
    pub fn slots(&self) -> u8 {
        self.slots
    }

    /// The job with this id, in the queue or in the history.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::queue::{JobId, Priority, Queue, Work};
    ///
    /// let mut queue = Queue::new();
    /// let (id, _) = queue.add(Work::Send { path: "a".into() }, Priority::Low)?;
    /// assert_eq!(queue.job(id).map(|job| job.priority()), Some(Priority::Low));
    /// assert!(queue.job(JobId(99)).is_none());
    /// # Ok::<(), sluice_core::Error>(())
    /// ```
    pub fn job(&self, id: JobId) -> Option<&Job> {
        self.jobs
            .get(&id)
            .or_else(|| self.history.iter().find(|job| job.id == id))
    }

    /// Every job that is not history, in the order they were added.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::queue::{Priority, Queue, Work};
    ///
    /// let mut queue = Queue::new();
    /// queue.add(Work::Send { path: "a".into() }, Priority::Low)?;
    /// queue.add(Work::Send { path: "b".into() }, Priority::High)?;
    /// let ids: Vec<u64> = queue.jobs().iter().map(|job| job.id().0).collect();
    /// assert_eq!(ids, [1, 2]);
    /// # Ok::<(), sluice_core::Error>(())
    /// ```
    pub fn jobs(&self) -> Vec<&Job> {
        self.jobs.values().collect()
    }

    /// The last 100 jobs that finished, failed or were cancelled, newest first.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::queue::{Priority, Queue, State, Work};
    ///
    /// let mut queue = Queue::new();
    /// let (id, _) = queue.add(Work::Send { path: "a".into() }, Priority::Normal)?;
    /// queue.cancel(id)?;
    /// assert_eq!(queue.history()[0].state(), State::Cancelled);
    /// # Ok::<(), sluice_core::Error>(())
    /// ```
    pub fn history(&self) -> Vec<&Job> {
        self.history.iter().collect()
    }

    /// Writes the queue to `db/queue` in `home`, through a temporary file and a rename, so a crash
    /// leaves the old queue or the new one and never half of either. The file is private to the
    /// user, because a ticket in a job is a read capability. A driver calls this whenever a command
    /// returns [`Action::Save`].
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::Home;
    /// use sluice_core::queue::{Priority, Queue, Work};
    ///
    /// let dir = tempfile::tempdir()?;
    /// let home = Home::open(dir.path())?;
    /// let mut queue = Queue::new();
    /// queue.add(Work::Send { path: "a.iso".into() }, Priority::Normal)?;
    /// queue.save(&home)?;
    /// assert!(dir.path().join("db").join("queue").is_file());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn save(&self, home: &Home) -> Result<(), Error> {
        save::write(self, home.path())
    }

    /// Reads the queue that [`Queue::save`] wrote in `home`, and returns it with the actions it
    /// starts with. A restart changes three things, because a transfer resumes from what is
    /// verified and loses nothing: a job that was running is in line again in its old place, a job
    /// that waited to retry is in line again with its count of failures, and the timers are gone.
    /// A paused job stays paused. No file is an empty queue.
    ///
    /// A file over 16 MiB, of another format version, or whose bytes do not decode to a queue that
    /// this one could have saved fails with [`Error::Storage`] naming the file, which is left as it
    /// is for the user to decide on.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::Home;
    /// use sluice_core::queue::{Action, Priority, Queue, State, Work};
    ///
    /// let dir = tempfile::tempdir()?;
    /// let home = Home::open(dir.path())?;
    /// let mut queue = Queue::new();
    /// let (id, _) = queue.add(Work::Send { path: "a.iso".into() }, Priority::Normal)?;
    /// queue.save(&home)?;
    /// let (loaded, actions) = Queue::load(&home)?;
    /// // No timer: a restart leaves no job waiting to retry.
    /// assert_eq!(actions, [Action::Start(id), Action::Save]);
    /// assert_eq!(loaded.job(id).unwrap().state(), State::Active);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn load(home: &Home) -> Result<(Queue, Vec<Action>), Error> {
        let mut queue = save::read(home.path())?;
        // A new process has no timer, and a load leaves no job waiting to retry, so this asks for
        // none.
        let actions = queue.settle(Vec::new(), false, None);
        Ok((queue, actions))
    }
}

/// The wait before the retry that follows network failure number `failures` in a row: 1 s, then
/// twice as long each time, up to 5 minutes. The 9th wait is 256 s, and the 10th to the 14th are
/// at the cap.
pub(crate) fn backoff_ms(failures: u32) -> u64 {
    // 2^20 seconds is far past the cap, and small enough that the shift cannot overflow.
    let doublings = failures.saturating_sub(1).min(20);
    (FIRST_WAIT_MS << doublings).min(MAX_WAIT_MS)
}

fn refuse(id: JobId, why: &str) -> Error {
    Error::Policy(format!("job {id} {why}"))
}

/// A path the queue takes: not empty, and short enough that the saved queue stays small.
fn check_path(work: &Work) -> Result<(), Error> {
    let units = path_units(work.path());
    if units == 0 {
        return Err(Error::Policy(
            "a job needs a path, and this one is empty".to_string(),
        ));
    }
    if units > MAX_PATH_UNITS {
        return Err(Error::Policy(format!(
            "the path of a job is at most {MAX_PATH_UNITS} bytes long, and this one is {units}"
        )));
    }
    Ok(())
}

#[cfg(unix)]
fn path_units(path: &std::path::Path) -> usize {
    use std::os::unix::ffi::OsStrExt;
    path.as_os_str().as_bytes().len()
}

#[cfg(windows)]
fn path_units(path: &std::path::Path) -> usize {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str().encode_wide().count()
}

#[cfg(not(any(unix, windows)))]
fn path_units(path: &std::path::Path) -> usize {
    path.as_os_str().len()
}
