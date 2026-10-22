//! What the queue holds: jobs, their priority and state, and the actions it asks its driver for.

use std::fmt;
use std::path::PathBuf;

use crate::ErrorKind;
use crate::ticket::Ticket;

/// The id of a job. Ids count up from 1 in the order jobs are added and are never used twice,
/// even across a restart, so an id is also a job's place in its line. A person types it in
/// decimal, as in `sluice pause 12`, which is how it displays.
///
/// # Examples
///
/// ```
/// use sluice_core::queue::JobId;
///
/// assert_eq!(JobId(12).to_string(), "12");
/// assert!(JobId(3) < JobId(12));
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct JobId(pub u64);

impl fmt::Display for JobId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Which line a job waits in. High always starts before normal, and normal before low.
///
/// # Examples
///
/// ```
/// use sluice_core::queue::Priority;
///
/// assert_eq!(Priority::default(), Priority::Normal);
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Priority {
    /// Starts before every normal and low job.
    High,
    /// The priority a job has unless it is given another.
    #[default]
    Normal,
    /// Starts only when no high or normal job is waiting.
    Low,
}

impl Priority {
    /// The line's place: 0 starts first.
    pub(super) fn rank(self) -> u8 {
        match self {
            Priority::High => 0,
            Priority::Normal => 1,
            Priority::Low => 2,
        }
    }
}

/// What a job does.
///
/// `Debug` hides the path and the ticket. A ticket is a read capability, and file names are kept
/// out of logs.
///
/// # Examples
///
/// ```
/// use sluice_core::queue::Work;
///
/// let work = Work::Send { path: "photos/2026".into() };
/// assert_eq!(format!("{work:?}"), "Send(<redacted>)");
/// ```
#[derive(Clone, PartialEq, Eq)]
pub enum Work {
    /// Send the file or folder at `path`.
    Send {
        /// What to send.
        path: PathBuf,
    },
    /// Fetch what `ticket` names and write it at `path`.
    Receive {
        /// What to fetch.
        ticket: Ticket,
        /// Where to write it.
        path: PathBuf,
    },
}

impl Work {
    pub(super) fn path(&self) -> &PathBuf {
        match self {
            Work::Send { path } | Work::Receive { path, .. } => path,
        }
    }
}

impl fmt::Debug for Work {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Work::Send { .. } => f.write_str("Send(<redacted>)"),
            Work::Receive { .. } => f.write_str("Receive(<redacted>)"),
        }
    }
}

/// Where a job is. The first four are the queue's; the last three are history.
///
/// # Examples
///
/// ```
/// use sluice_core::ErrorKind;
/// use sluice_core::queue::State;
///
/// assert!(!State::Waiting.is_history());
/// assert!(State::Failed(ErrorKind::Verify).is_history());
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// In line for a free slot.
    Waiting,
    /// Running in a slot.
    Active,
    /// Kept from starting until it is resumed.
    Paused,
    /// Failed on the network and waits until `at_ms`, on the caller's monotonic clock, before it
    /// is in line again.
    Retrying {
        /// When the wait ends.
        at_ms: u64,
    },
    /// Finished.
    Done,
    /// Failed for good, with the kind of the last failure.
    Failed(ErrorKind),
    /// Cancelled by the user.
    Cancelled,
}

impl State {
    /// Whether the job is over: done, failed or cancelled.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::queue::State;
    ///
    /// assert!(State::Done.is_history());
    /// assert!(!State::Retrying { at_ms: 5 }.is_history());
    /// ```
    pub fn is_history(self) -> bool {
        matches!(self, State::Done | State::Failed(_) | State::Cancelled)
    }
}

/// One transfer in the queue or in its history.
///
/// # Examples
///
/// ```
/// use sluice_core::queue::{JobId, Priority, Queue, State, Work};
///
/// let mut queue = Queue::new();
/// let (id, _) = queue.add(Work::Send { path: "a.iso".into() }, Priority::High)?;
/// let job = queue.job(id).expect("just added");
/// assert_eq!((job.id(), job.priority(), job.state()), (JobId(1), Priority::High, State::Active));
/// assert_eq!(job.network_failures(), 0);
/// assert!(matches!(job.work(), Work::Send { .. }));
/// # Ok::<(), sluice_core::Error>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Job {
    pub(super) id: JobId,
    pub(super) work: Work,
    pub(super) priority: Priority,
    pub(super) state: State,
    pub(super) failures: u32,
}

impl Job {
    /// The job's id, which is also its place in its line.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::queue::{Priority, Queue, Work};
    ///
    /// let mut queue = Queue::new();
    /// let (id, _) = queue.add(Work::Send { path: "a".into() }, Priority::Low)?;
    /// let job = queue.job(id).expect("just added");
    /// assert_eq!(job.id(), id);
    /// # Ok::<(), sluice_core::Error>(())
    /// ```
    pub fn id(&self) -> JobId {
        self.id
    }

    /// What the job does.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::queue::{Priority, Queue, Work};
    ///
    /// let mut queue = Queue::new();
    /// let (id, _) = queue.add(Work::Send { path: "a".into() }, Priority::Low)?;
    /// let job = queue.job(id).expect("just added");
    /// assert!(matches!(job.work(), Work::Send { .. }));
    /// # Ok::<(), sluice_core::Error>(())
    /// ```
    pub fn work(&self) -> &Work {
        &self.work
    }

    /// The line the job waits in.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::queue::{Priority, Queue, Work};
    ///
    /// let mut queue = Queue::new();
    /// let (id, _) = queue.add(Work::Send { path: "a".into() }, Priority::Low)?;
    /// let job = queue.job(id).expect("just added");
    /// assert_eq!(job.priority(), Priority::Low);
    /// # Ok::<(), sluice_core::Error>(())
    /// ```
    pub fn priority(&self) -> Priority {
        self.priority
    }

    /// Where the job is.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::queue::{Priority, Queue, Work};
    ///
    /// let mut queue = Queue::new();
    /// let (id, _) = queue.add(Work::Send { path: "a".into() }, Priority::Low)?;
    /// let job = queue.job(id).expect("just added");
    /// assert_eq!(job.state(), sluice_core::queue::State::Active);
    /// # Ok::<(), sluice_core::Error>(())
    /// ```
    pub fn state(&self) -> State {
        self.state
    }

    /// How many network failures in a row the job has had since it last made progress.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::queue::{Priority, Queue, Work};
    ///
    /// let mut queue = Queue::new();
    /// let (id, _) = queue.add(Work::Send { path: "a".into() }, Priority::Low)?;
    /// let job = queue.job(id).expect("just added");
    /// assert_eq!(job.network_failures(), 0);
    /// # Ok::<(), sluice_core::Error>(())
    /// ```
    pub fn network_failures(&self) -> u32 {
        self.failures
    }

    /// Where the job goes in the order jobs start: by line, then first in, first out.
    pub(super) fn place(&self) -> (u8, JobId) {
        (self.priority.rank(), self.id)
    }
}

/// What the driver of a queue does after a command. They come in the order to do them: stops,
/// then starts, then a save, then the timer.
///
/// # Examples
///
/// ```
/// use sluice_core::queue::{Action, JobId, Priority, Queue, Work};
///
/// let mut queue = Queue::new();
/// let (id, actions) = queue.add(Work::Send { path: "a.iso".into() }, Priority::Normal)?;
/// assert_eq!(actions, [Action::Start(id), Action::Save]);
/// assert_eq!(queue.pause(id)?, [Action::Stop(JobId(1)), Action::Save]);
/// # Ok::<(), sluice_core::Error>(())
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Stop the transfer of this job. It was paused or cancelled while it ran.
    Stop(JobId),
    /// Start the transfer of this job.
    Start(JobId),
    /// Write the queue to its file, because something that a restart keeps has changed.
    Save,
    /// Set the driver's one timer. `Some(at)` replaces any time asked for before: call
    /// [`Queue::tick`](super::Queue::tick) at `at` on the caller's monotonic clock, which is when
    /// the next retry is due. `None` clears the timer, because no job waits to retry. It comes only
    /// when the time of the earliest retry has changed, and a tick counts as the timer going off,
    /// so a tick that leaves a job waiting always sets the timer again.
    Wake {
        /// When to tick, in milliseconds, or `None` for no timer.
        at_ms: Option<u64>,
    },
}
