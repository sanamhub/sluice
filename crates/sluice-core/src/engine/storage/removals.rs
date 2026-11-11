//! The removals one command has started, so the command can wait for them before it ends.
//!
//! A removal waits for a file another process may still hold, and those waits add up to more than a
//! second. What happens to one when the command's runtime is dropped is not one thing: a blocking
//! task that is already running is waited for, without limit, because no shutdown timeout is set; a
//! blocking task that has not started is dropped, and [`Queued`] keeps its path for the end of the
//! command; and a thread of our own, the one a dropped [`super::Temp`] starts, is never joined at
//! all. A command that ended during one left its folder behind, for a sweep that only removes a
//! folder a day old. So every removal is counted here, and a command waits for the count to reach
//! zero, and tries again for the folders a hold kept, before it exits.

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
/// `budget` at the latest plus the pass it is in. Anything still there is left to the sweep of
/// temporary folders.
///
/// A command calls this on every way out, after its runtime is gone; an embedder calls the same
/// function. No pass waits for anything: a path's hold is taken only when it is free, and a path
/// whose hold is held, or which a record of this home names, is kept for the next tick. The records
/// are read from the folder [`Home::partial_records_for_read`] names, which is resolved once and
/// never made, so a command that never used this home learns nothing about it and writes nothing
/// there.
///
/// A pass that begins before the budget is gone retries every kept path once, and a pass that
/// begins after it makes one attempt and then ends the wait: nothing it does can take longer than
/// one removal, and none of it waits on a hold.
///
/// [`Home::partial_records_for_read`]: crate::Home::partial_records_for_read
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
    passes(budget, crate::Home::partial_records_for_read());
}

/// [`wait_for_removals`] with the records folder named by the caller, so a test's passes read a
/// folder of its own and never the user's home.
#[cfg(test)]
pub(crate) fn wait_for_removals_at(budget: Duration, records: Option<PathBuf>) {
    passes(budget, records);
}

/// [`wait_for_removals`] with the folder the records are read from, which is resolved once here.
fn passes(budget: Duration, records: Option<PathBuf>) {
    let deadline = Instant::now() + budget;
    // A pass is made for what is already kept even when waiting for the running removals used the
    // whole budget. A path that was kept before the wait would otherwise be left for the sweep
    // without a single try, which is what a command whose own removals were still parked did: it
    // left the folder for a day.
    wait_for_running(deadline);
    loop {
        let paths = std::mem::take(&mut *left_list());
        if paths.is_empty() {
            return;
        }
        let spent = Instant::now() >= deadline;
        let dir = records.as_deref();
        // The folders this home has a record for, read once for the pass: a folder a record names
        // is a claim's, and removing it would take a store out from under a receive.
        let named = dir.map(super::registry::recorded_in).unwrap_or_default();
        #[cfg(test)]
        seam::begin(spent);
        let mut still = Vec::new();
        for path in paths {
            let survives = if spent {
                try_still_there(&path, &named, dir) != Some(false)
            } else {
                retry(&path, &named, dir)
            };
            if survives {
                still.push(path);
            }
        }
        #[cfg(test)]
        seam::before_merge();
        // Appending, not assigning: a `Temp::drop` thread or a late `Queued` drop can add a path
        // while this pass runs, and replacing the list would lose it.
        let mut kept = left_list();
        for path in still {
            if !kept.contains(&path) {
                kept.push(path);
            }
        }
        drop(kept);
        #[cfg(test)]
        seam::after_merge();
        if spent || Instant::now() >= deadline {
            return;
        }
        std::thread::sleep(RETRY_EVERY);
    }
}

/// One more try at a kept path, for a pass whose budget is gone. `true` means the path is still
/// there and `None` means it was not touched: either another removal holds it, or a record of this
/// home names it, and the next wait or the sweep is what tries it again.
fn try_still_there(path: &Path, named: &[PathBuf], records: Option<&Path>) -> Option<bool> {
    attempt(path, named, records)
}

/// One more try at a kept path, for a pass inside the budget. It waits for nothing either: a path
/// another removal holds is kept for the next tick rather than waited on, so no pass can outlast
/// the budget by a hold.
fn retry(path: &Path, named: &[PathBuf], records: Option<&Path>) -> bool {
    attempt(path, named, records) != Some(false)
}

/// One try at removing `path`, under its hold when the hold is free.
///
/// A path a record names is a claim's and is left alone. A partial store is the one folder a claim
/// can make between the records being read and the hold being taken, so it is asked about again
/// under the hold: that read is what makes removing it safe, and it is cheap because only a folder
/// named like a store is asked about twice. A note that stands for the folder goes with it, so a
/// later claim of the output does not take the name away from whatever is there by then.
fn attempt(path: &Path, named: &[PathBuf], records: Option<&Path>) -> Option<bool> {
    let outcome = super::registry::try_with_hold(path, || {
        if names(path, named) {
            return false;
        }
        if super::store::is_partial(path) && super::registry::named_in(records, path) {
            return false;
        }
        match super::remove_once(path) {
            Ok(()) => {
                super::registry::forget_note(records, path);
                false
            }
            Err(_) => true,
        }
    });
    match outcome {
        Ok(Some(still)) => Some(still),
        Ok(None) => None,
        // A path whose folder cannot be resolved has no folder to remove.
        Err(_) => Some(false),
    }
}

/// Whether `path` is one of `named`, the folders this home had a record for when the pass began.
fn names(path: &Path, named: &[PathBuf]) -> bool {
    super::registry::canonical(path).is_ok_and(|wanted| named.contains(&wanted))
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

/// The paths the end of a command would still try, for a test that watches them.
#[cfg(test)]
pub(super) fn kept_paths() -> Vec<PathBuf> {
    left_list().clone()
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

/// The moments of one retry pass a test can act at, so it can watch a path arrive while a pass
/// runs, or count the passes a spent budget allows, without a sleep. One test uses them at a time,
/// under [`exclusive`].
#[cfg(test)]
pub(super) mod seam {
    use std::{
        sync::MutexGuard,
        sync::{Arc, Mutex},
    };

    type Run = Box<dyn FnOnce() + Send>;
    type Watcher = Arc<dyn Fn(bool) + Send + Sync>;

    static BEFORE: Mutex<Option<Run>> = Mutex::new(None);
    static AFTER: Mutex<Option<Run>> = Mutex::new(None);
    static WATCHED: Mutex<Option<Watcher>> = Mutex::new(None);

    /// Arms the next pass: `before` runs after every path is retried and before the survivors go
    /// back, and `after` runs once they are.
    pub(crate) fn arm(
        before: impl FnOnce() + Send + 'static,
        after: impl FnOnce() + Send + 'static,
    ) {
        *BEFORE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Box::new(before));
        *AFTER
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Box::new(after));
    }

    /// Watches every pass that begins from now on. The returned value stops the watch when it is
    /// dropped, whether the test ended or panicked, so a watcher of one test never sees another's
    /// passes.
    pub(crate) fn watch(observer: impl Fn(bool) + Send + Sync + 'static) -> Watched {
        *watched() = Some(Arc::new(observer));
        Watched(())
    }

    /// Tells the watcher that a pass began, and whether the budget was gone when it did.
    pub(super) fn begin(spent: bool) {
        let observer = watched().as_ref().map(Arc::clone);
        if let Some(observer) = observer {
            observer(spent);
        }
    }

    pub(super) fn before_merge() {
        take(&BEFORE)();
    }

    pub(super) fn after_merge() {
        take(&AFTER)();
    }

    /// Ends the watch of [`watch`] when it is dropped.
    pub(crate) struct Watched(());

    impl Drop for Watched {
        fn drop(&mut self) {
            *watched() = None;
        }
    }

    fn watched() -> MutexGuard<'static, Option<Watcher>> {
        WATCHED
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn take(slot: &Mutex<Option<Run>>) -> Run {
        slot.lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
            .unwrap_or_else(|| Box::new(|| {}))
    }
}
