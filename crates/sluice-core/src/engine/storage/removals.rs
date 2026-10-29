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

/// A removal handed to a thread that may never run it. A blocking task the runtime drops before it
/// starts takes its closure with it, and a plain count would go to zero without the path being
/// removed or kept: the folder would stay until the sweep. Dropping this value before it ran records
/// the path as left, so the end of the command tries it again.
pub(super) struct Queued {
    path: PathBuf,
    ran: bool,
    _doing: Doing,
}

impl Queued {
    /// Counts a removal from this moment, before it is handed to a thread.
    pub(super) fn begin(path: PathBuf) -> Self {
        Self {
            path,
            ran: false,
            _doing: Doing::begin(),
        }
    }

    /// Runs the removal. The count is released only after it, so a waiter sees it as running for as
    /// long as it is.
    pub(super) fn run(mut self) {
        self.ran = true;
        super::remove_quietly(&self.path);
    }
}

impl Drop for Queued {
    fn drop(&mut self) {
        if !self.ran {
            left(&self.path);
        }
    }
}

/// Notes that `path` is still there after a removal, so the end of the command tries it again. The
/// path is kept in the one form the hold uses, so a later removal that spells it another way clears
/// the same entry. A path that cannot be resolved is not kept: no removal of it could be tried.
pub(super) fn left(path: &Path) {
    let Ok(key) = super::registry::canonical(path) else {
        return;
    };
    let mut left = left_list();
    if !left.contains(&key) {
        left.push(key);
    }
}

/// Notes that `path` is gone, so the end of the command does not look for it again.
pub(super) fn gone(path: &Path) {
    let Ok(key) = super::registry::canonical(path) else {
        return;
    };
    left_list().retain(|at| *at != key);
}

/// Waits until no removal is running, then tries again for the paths a hold kept, and returns after
/// `budget` at the latest. Anything still there is left to the sweep of temporary folders.
///
/// A command calls this on every way out, after its runtime is gone; an embedder calls the same
/// function. A retry takes the path's hold and asks whether a record of this home names it: a claim
/// may have made it a live store again, under another spelling of the same folder, since the removal
/// gave up.
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
            if still_there(&path) {
                still.push(path);
            }
        }
        *left_list() = still;
        std::thread::sleep(RETRY_EVERY);
    }
}

/// One more try at a kept path, under its hold, unless a record names it as a store this home still
/// has. `true` means it is still there.
fn still_there(path: &Path) -> bool {
    super::registry::with_hold(path, || {
        if super::registry::named(path) {
            // A claim has made this a live store since the removal gave up; its folder is the
            // claim's, and the record it wrote says so.
            return false;
        }
        super::remove_once(path).is_err()
    })
    // Without a resolved form no hold can be taken, and no removal of it can be tried.
    .unwrap_or(true)
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

/// The one lock a test takes while it watches the removals of the whole process, because the count
/// and the kept paths are shared by every test in one binary: under `cargo test` a parked removal of
/// another test would use up the budget of one that waits for them, and its folder could be retried
/// while it is still there. A test run that gives every test its own process does not need it, and
/// taking it costs nothing there.
#[cfg(test)]
pub(crate) fn exclusive() -> MutexGuard<'static, ()> {
    static ONE: Mutex<()> = Mutex::new(());
    ONE.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}
