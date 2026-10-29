//! The removals one command has started, so the command can wait for them before it ends.
//!
//! A removal waits for a file another process may still hold, and those waits add up to more than a
//! second. They run on threads that may block, and neither a blocking task nor a thread of our own
//! is joined when the command's runtime is dropped: a `spawn_blocking` task outlives the future
//! that started it. A command that ended during one left its folder behind, for a sweep that only
//! removes a folder a day old. So every removal is counted here, and a command waits for the count
//! to reach zero, and tries again for the folders a hold kept, before it exits.

use std::{
    path::{Path, PathBuf},
    sync::{Condvar, Mutex, MutexGuard},
    time::{Duration, Instant},
};

/// How often the folders that could not be removed are tried again at the end of a command. Short
/// enough to fit many tries into a moment, long enough not to spin.
const RETRY_EVERY: Duration = Duration::from_millis(10);

static RUNNING: Mutex<usize> = Mutex::new(0);
static FINISHED: Condvar = Condvar::new();
static LEFT: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

/// One removal while it is happening. Making one says the removal has started, and dropping it says
/// it is over, whether the path is gone or not.
pub(super) struct Doing(());

impl Doing {
    /// Counts a removal from this moment, so a command that ends waits for it even if the thread
    /// that does it has not been scheduled yet.
    pub(super) fn begin() -> Self {
        *running() += 1;
        Self(())
    }
}

impl Drop for Doing {
    fn drop(&mut self) {
        *running() -= 1;
        FINISHED.notify_all();
    }
}

/// Notes that `path` is still there after a removal, so the end of the command tries it again. A
/// path is kept once, however often it fails.
pub(super) fn left(path: &Path) {
    let mut left = left_list();
    if !left.iter().any(|at| at == path) {
        left.push(path.to_owned());
    }
}

/// Notes that `path` is gone, so the end of the command does not look for it again.
pub(super) fn gone(path: &Path) {
    left_list().retain(|at| at != path);
}

/// Waits until no removal is running, then tries again for the paths a hold kept, and returns after
/// `budget` at the latest. Anything still there is left to the sweep of temporary folders.
///
/// A command calls this on every way out, after its runtime is gone; an embedder calls the same
/// function. The runtime's own drop does not wait for the blocking tasks it started, so a process
/// that ends without this leaves folders nothing removes until the sweep runs.
///
/// # Examples
///
/// ```
/// use std::time::Duration;
///
/// // Nothing is being removed, so this returns at once.
/// sluice_core::engine::wait_for_removals(Duration::from_secs(1));
/// ```
pub fn wait_for_removals(budget: Duration) {
    let deadline = Instant::now() + budget;
    wait_for_running(deadline);
    loop {
        let paths = std::mem::take(&mut *left_list());
        if paths.is_empty() || Instant::now() >= deadline {
            return;
        }
        let mut still = Vec::new();
        for path in paths {
            if super::remove_once(&path).is_err() {
                still.push(path);
            }
        }
        *left_list() = still;
        std::thread::sleep(RETRY_EVERY);
    }
}

/// Waits for the removals that are running, or until `deadline` passes.
fn wait_for_running(deadline: Instant) {
    let mut running = running();
    while *running > 0 {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return;
        }
        running = FINISHED
            .wait_timeout(running, left)
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .0;
    }
}

fn running() -> MutexGuard<'static, usize> {
    RUNNING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn left_list() -> MutexGuard<'static, Vec<PathBuf>> {
    LEFT.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}
