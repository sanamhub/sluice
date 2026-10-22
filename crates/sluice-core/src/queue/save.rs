//! The queue on disk: `db/queue` in the store home.
//!
//! The file is one version byte, then `postcard` of [`Saved`]. Its own types, and not the queue's,
//! go through serde, so the bytes change only when this file does. Each job is saved as it is, and
//! the rules of a restart apply when it is loaded: a deadline means nothing to a new clock.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{
    HISTORY, Job, JobId, MAX_JOBS, MAX_NETWORK_FAILURES, MAX_PATH_UNITS, MAX_SLOTS, Priority,
    Queue, State, Work, path_units,
};
use crate::home::write_atomic_secret;
use crate::ticket::Ticket;
use crate::{Error, ErrorKind};

/// The only version of the file this build reads and writes.
const VERSION: u8 = 1;
/// The largest file a load reads. The most a queue can hold is about 13.5 MB.
const MAX_FILE_BYTES: u64 = 16 << 20;

#[derive(Serialize, Deserialize)]
struct Saved {
    next_id: u64,
    slots: u8,
    /// Every job that is not history, by id.
    jobs: Vec<SavedJob>,
    /// Newest first.
    history: Vec<SavedJob>,
}

#[derive(Serialize, Deserialize)]
struct SavedJob {
    id: u64,
    work: SavedWork,
    priority: SavedPriority,
    state: SavedState,
    failures: u32,
}

#[derive(Serialize, Deserialize)]
enum SavedWork {
    Send { path: Vec<u8> },
    Receive { ticket: String, path: Vec<u8> },
}

#[derive(Serialize, Deserialize)]
enum SavedPriority {
    High,
    Normal,
    Low,
}

/// A wait to retry is saved without its deadline, which was on the clock of another process.
#[derive(Serialize, Deserialize)]
enum SavedState {
    Waiting,
    Active,
    Paused,
    Retrying,
    Done,
    Failed(SavedKind),
    Cancelled,
}

#[derive(Serialize, Deserialize)]
enum SavedKind {
    Network,
    Verify,
    Storage,
    Policy,
    Cancelled,
    Bug,
}

/// Where the queue lives in a home.
pub(super) fn file_in(home: &Path) -> PathBuf {
    home.join("db").join("queue")
}

pub(super) fn write(queue: &Queue, home: &Path) -> Result<(), Error> {
    // Secret, because a ticket in a job is a read capability.
    write_atomic_secret(&home.join("db"), "queue", &encode(queue)?)
}

/// The queue in `home`, an empty one if there is no file, or [`Error::Storage`] naming the file
/// and leaving it as it is.
pub(super) fn read(home: &Path) -> Result<Queue, Error> {
    let path = file_in(home);
    let file = match File::open(&path) {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Queue::new()),
        Err(e) => return Err(unreadable(&path, &format!("it cannot be opened: {e}"))),
    };
    let mut bytes = Vec::new();
    // One byte past the limit, to tell a file at the limit from one over it.
    file.take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| unreadable(&path, &format!("it cannot be read: {e}")))?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(unreadable(&path, "it is over 16 MiB"));
    }
    decode(&bytes).map_err(|why| unreadable(&path, &why))
}

fn unreadable(path: &Path, why: &str) -> Error {
    Error::Storage(format!(
        "the transfer queue in {} is not one this sluice can load: {why}. The file was left as \
         it is. Move it elsewhere to start with an empty queue",
        path.display()
    ))
}

pub(super) fn encode(queue: &Queue) -> Result<Vec<u8>, Error> {
    let saved = Saved {
        next_id: queue.next_id,
        slots: queue.slots,
        jobs: queue.jobs.values().map(save_job).collect(),
        history: queue.history.iter().map(save_job).collect(),
    };
    let mut bytes = vec![VERSION];
    let body = postcard::to_allocvec(&saved)
        .map_err(|e| Error::Bug(format!("the queue does not encode: {e}")))?;
    bytes.extend_from_slice(&body);
    Ok(bytes)
}

/// The queue in `bytes`, or why they are not one. A reason never quotes a path or a ticket.
pub(super) fn decode(bytes: &[u8]) -> Result<Queue, String> {
    let (&version, body) = bytes.split_first().ok_or("it is empty")?;
    if version != VERSION {
        return Err(format!(
            "its format is version {version}, and this sluice reads {VERSION}"
        ));
    }
    let (saved, rest): (Saved, _) =
        postcard::take_from_bytes(body).map_err(|e| format!("it does not decode: {e}"))?;
    if !rest.is_empty() {
        return Err("it has bytes after the queue".to_string());
    }
    if saved.next_id == 0 {
        return Err("its next job id is 0, and ids start at 1".to_string());
    }
    if !(1..=MAX_SLOTS).contains(&saved.slots) {
        return Err(format!(
            "its slot count is {}, not from 1 to {MAX_SLOTS}",
            saved.slots
        ));
    }
    if saved.jobs.len() > MAX_JOBS || saved.history.len() > HISTORY {
        return Err("it holds more jobs than a queue takes".to_string());
    }
    let mut seen = BTreeSet::new();
    let mut jobs = BTreeMap::new();
    for saved_job in saved.jobs {
        let job = load_job(saved_job, saved.next_id, &mut seen, false)?;
        jobs.insert(job.id, job);
    }
    let mut history = VecDeque::new();
    for saved_job in saved.history {
        history.push_back(load_job(saved_job, saved.next_id, &mut seen, true)?);
    }
    Ok(Queue {
        jobs,
        history,
        next_id: saved.next_id,
        slots: saved.slots,
    })
}

fn save_job(job: &Job) -> SavedJob {
    let work = match &job.work {
        Work::Send { path } => SavedWork::Send {
            path: path_bytes(path),
        },
        Work::Receive { ticket, path } => SavedWork::Receive {
            ticket: ticket.encode(),
            path: path_bytes(path),
        },
    };
    let priority = match job.priority {
        Priority::High => SavedPriority::High,
        Priority::Normal => SavedPriority::Normal,
        Priority::Low => SavedPriority::Low,
    };
    SavedJob {
        id: job.id.0,
        work,
        priority,
        state: save_state(job.state),
        failures: job.failures,
    }
}

fn save_state(state: State) -> SavedState {
    match state {
        State::Waiting => SavedState::Waiting,
        State::Active => SavedState::Active,
        State::Paused => SavedState::Paused,
        State::Retrying { .. } => SavedState::Retrying,
        State::Done => SavedState::Done,
        State::Failed(kind) => SavedState::Failed(match kind {
            ErrorKind::Network => SavedKind::Network,
            ErrorKind::Verify => SavedKind::Verify,
            ErrorKind::Storage => SavedKind::Storage,
            ErrorKind::Policy => SavedKind::Policy,
            ErrorKind::Cancelled => SavedKind::Cancelled,
            ErrorKind::Bug => SavedKind::Bug,
        }),
        State::Cancelled => SavedState::Cancelled,
    }
}

/// A job from the file, checked as the queue would have checked it, with the rules of a restart
/// applied: a job that ran or waited to retry is in line again, with its count of failures.
fn load_job(
    saved: SavedJob,
    next_id: u64,
    seen: &mut BTreeSet<u64>,
    in_history: bool,
) -> Result<Job, String> {
    let id = saved.id;
    if id == 0 || id >= next_id || !seen.insert(id) {
        return Err(format!(
            "job {id} has an id that is out of order or used twice"
        ));
    }
    let state = load_state(saved.state);
    let failures_ok = if in_history {
        saved.failures <= MAX_NETWORK_FAILURES
    } else {
        saved.failures < MAX_NETWORK_FAILURES
    };
    if state.is_history() != in_history || !failures_ok {
        return Err(format!(
            "job {id} is in a state the queue never leaves it in"
        ));
    }
    let work = match saved.work {
        SavedWork::Send { path } => Work::Send {
            path: load_path(path, id)?,
        },
        SavedWork::Receive { ticket, path } => Work::Receive {
            ticket: Ticket::decode(&ticket).map_err(|_| format!("job {id} has a bad ticket"))?,
            path: load_path(path, id)?,
        },
    };
    let priority = match saved.priority {
        SavedPriority::High => Priority::High,
        SavedPriority::Normal => Priority::Normal,
        SavedPriority::Low => Priority::Low,
    };
    Ok(Job {
        id: JobId(id),
        work,
        priority,
        state,
        failures: saved.failures,
    })
}

/// The state a restart leaves a job in.
fn load_state(state: SavedState) -> State {
    match state {
        SavedState::Waiting | SavedState::Active | SavedState::Retrying => State::Waiting,
        SavedState::Paused => State::Paused,
        SavedState::Done => State::Done,
        SavedState::Failed(kind) => State::Failed(match kind {
            SavedKind::Network => ErrorKind::Network,
            SavedKind::Verify => ErrorKind::Verify,
            SavedKind::Storage => ErrorKind::Storage,
            SavedKind::Policy => ErrorKind::Policy,
            SavedKind::Cancelled => ErrorKind::Cancelled,
            SavedKind::Bug => ErrorKind::Bug,
        }),
        SavedState::Cancelled => State::Cancelled,
    }
}

fn load_path(bytes: Vec<u8>, id: u64) -> Result<PathBuf, String> {
    let path = path_from_bytes(bytes).ok_or_else(|| format!("job {id} has a malformed path"))?;
    let units = path_units(&path);
    if units == 0 || units > MAX_PATH_UNITS {
        return Err(format!("job {id} has a path of {units} units"));
    }
    Ok(path)
}

/// The path as the platform holds it, so a name that is not valid Unicode survives: the bytes on
/// Unix, and the UTF-16 units, little-endian, on Windows. A queue file stays on the machine that
/// wrote it, like the rest of the home.
#[cfg(unix)]
fn path_bytes(path: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    path.as_os_str().as_bytes().to_vec()
}

#[cfg(unix)]
fn path_from_bytes(bytes: Vec<u8>) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    Some(PathBuf::from(std::ffi::OsString::from_vec(bytes)))
}

#[cfg(windows)]
fn path_bytes(path: &Path) -> Vec<u8> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str()
        .encode_wide()
        .flat_map(u16::to_le_bytes)
        .collect()
}

#[cfg(windows)]
fn path_from_bytes(bytes: Vec<u8>) -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    let (pairs, odd) = bytes.as_chunks::<2>();
    if !odd.is_empty() {
        return None;
    }
    let units: Vec<u16> = pairs.iter().map(|pair| u16::from_le_bytes(*pair)).collect();
    Some(PathBuf::from(std::ffi::OsString::from_wide(&units)))
}

#[cfg(not(any(unix, windows)))]
fn path_bytes(path: &Path) -> Vec<u8> {
    path.as_os_str().as_encoded_bytes().to_vec()
}

#[cfg(not(any(unix, windows)))]
fn path_from_bytes(bytes: Vec<u8>) -> Option<PathBuf> {
    String::from_utf8(bytes).ok().map(PathBuf::from)
}
