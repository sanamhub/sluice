//! The private temporary folder one command writes a store in: its name, the lock that says a
//! running command holds it, and the sweep that removes what a killed command left behind.
//!
//! A command that is killed cannot remove anything, so its folder stays. Age alone cannot tell that
//! folder from one a command is using right now, so each folder holds a lock for as long as its
//! command lives: the system drops it when that process ends, and the sweep takes it to prove that
//! nobody holds the folder before removing it.

use std::{
    fs::{self, File},
    io,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use super::{remove_quietly, storage};
use crate::{Error, Home};

/// The prefix of every temporary folder, so the sweep removes nothing else.
pub const PREFIX: &str = ".sluice-tmp-";

/// The lock file inside a temporary folder. The system releases the lock when the process holding
/// it ends, so a lock that can be taken says no running command holds the folder.
const LOCK: &str = "lock";

/// How long a temporary folder nobody holds is left before a command removes it. A day is long
/// enough that no command still needs one and short enough that a killed one does not keep its
/// disk for ever.
pub const STALE_AFTER: Duration = Duration::from_secs(24 * 60 * 60);

/// A temporary folder for one store, with the lock that keeps the sweep away from it. The folder is
/// removed and the lock released when the value is dropped.
pub struct Temp {
    path: PathBuf,
    /// Held for as long as this value lives.
    _lock: File,
}

impl Temp {
    /// The folder itself.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        remove_quietly(&self.path);
    }
}

/// Makes a temporary folder in `out` with one `create_dir`, so it cannot follow a link that someone
/// left under a fixed name, and takes the lock that tells the sweep it is in use. The caller removes
/// it by dropping the value.
pub fn temp_dir(out: &Path) -> Result<Temp, Error> {
    let mut last = None;
    for _ in 0..8 {
        let mut bytes = [0u8; 8];
        if let Err(e) = getrandom::fill(&mut bytes) {
            return Err(Error::Storage(format!(
                "cannot get a random name for a temporary folder: {e}"
            )));
        }
        let path = out.join(format!("{PREFIX}{:016x}", u64::from_le_bytes(bytes)));
        match fs::create_dir(&path) {
            Ok(()) => return hold(path),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => last = Some(e),
            Err(e) => return Err(storage("create", &path, e)),
        }
    }
    Err(storage(
        "create a temporary folder in",
        out,
        last.unwrap_or_else(|| io::Error::other("no name was free")),
    ))
}

/// Takes the lock of a folder that was just made. A lock that cannot be taken on a folder with no
/// other holder is a file system that cannot lock, and that is not a reason to fail a transfer: the
/// folder is still removed when its command ends, and the sweep leaves it a day longer.
fn hold(path: PathBuf) -> Result<Temp, Error> {
    let lock = File::create(path.join(LOCK)).map_err(|e| storage("create", &path, e))?;
    if lock.try_lock().is_err() {
        // Left unlocked on purpose rather than failing: see above.
    }
    Ok(Temp { path, _lock: lock })
}

/// A name inside the temporary folder that no entry can have, because the folder is new and only
/// this writer names anything in it.
pub fn temp_path(dir: &Path, index: u64) -> PathBuf {
    dir.join(format!("{index}"))
}

/// Removes the temporary folders in the home's `tmp` that no running command holds and that are
/// older than [`STALE_AFTER`]. A command calls this when it starts, so a killed one is cleaned up by
/// the next command rather than by a sweep of the output folder, which is never done by name.
pub fn sweep_temporary_store(now: SystemTime) -> Result<(), Error> {
    sweep(&Home::temporary_store()?, now)
}

/// [`sweep_temporary_store`] in `parent`, with the clock as an argument so a test does not wait a
/// day.
fn sweep(parent: &Path, now: SystemTime) -> Result<(), Error> {
    for entry in fs::read_dir(parent).map_err(|e| storage("list", parent, e))? {
        let entry = entry.map_err(|e| storage("list", parent, e))?;
        if !entry.file_name().to_string_lossy().starts_with(PREFIX) {
            continue;
        }
        let path = entry.path();
        let meta = match fs::symlink_metadata(&path) {
            Ok(meta) => meta,
            Err(_) => continue,
        };
        if !meta.is_dir() || meta.file_type().is_symlink() {
            continue;
        }
        let old = meta
            .modified()
            .ok()
            .and_then(|at| now.duration_since(at).ok())
            .is_some_and(|age| age >= STALE_AFTER);
        if old && !held(&path) {
            remove_quietly(&path);
        }
    }
    Ok(())
}

/// Whether a running command still holds `path`. Reading the lock file takes no lock; trying to
/// lock it succeeds only when nobody holds it. A folder with no lock file has no holder either: this
/// version writes one into every temporary folder it makes, and the `tmp` folder is private to this
/// user, so nothing else can have put a folder there.
fn held(path: &Path) -> bool {
    let Ok(lock) = File::open(path.join(LOCK)) else {
        return false;
    };
    match lock.try_lock() {
        Ok(()) => {
            let _ = lock.unlock();
            false
        }
        Err(_) => true,
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        time::{Duration, SystemTime},
    };

    use super::{PREFIX, STALE_AFTER, sweep, temp_dir};

    /// A folder a live command holds is never swept, however old it looks: the lock says it is in
    /// use, and that is the only proof there is.
    #[test]
    fn the_sweep_removes_an_old_folder_nobody_holds_and_keeps_one_that_is_held() {
        let dir = tempfile::tempdir().unwrap();
        let held = temp_dir(dir.path()).unwrap();
        let later = SystemTime::now() + STALE_AFTER + Duration::from_secs(1);

        sweep(dir.path(), SystemTime::now()).unwrap();
        assert!(held.path().is_dir(), "a fresh folder was swept");

        sweep(dir.path(), later).unwrap();
        assert!(held.path().is_dir(), "a held folder was swept");

        // The same age, but nobody holds it: a command that was killed left it.
        let left = temp_dir(dir.path()).unwrap();
        let left_path = left.path().to_path_buf();
        drop(left);
        fs::create_dir(&left_path).unwrap();
        fs::write(left_path.join("store"), b"bytes").unwrap();
        sweep(dir.path(), later).unwrap();
        assert!(!left_path.exists(), "an old folder nobody held was kept");
    }

    /// Only folders with the temporary prefix are looked at: the sweep never removes the home's
    /// other entries, and never walks into anything.
    #[test]
    fn the_sweep_looks_only_at_the_temporary_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let other = dir.path().join("blobs");
        fs::create_dir(&other).unwrap();
        let named = dir.path().join(format!("{PREFIX}named"));
        fs::create_dir(&named).unwrap();
        let later = SystemTime::now() + STALE_AFTER + Duration::from_secs(1);
        sweep(dir.path(), later).unwrap();
        assert!(other.is_dir(), "an entry without the prefix was swept");
        assert!(!named.exists(), "an old temporary folder was kept");
    }

    /// Dropping the folder's value removes it, which is what a finished or cancelled command needs.
    #[test]
    fn a_dropped_folder_is_removed() {
        let dir = tempfile::tempdir().unwrap();
        let folder = temp_dir(dir.path()).unwrap();
        let path = folder.path().to_path_buf();
        assert!(path.is_dir());
        drop(folder);
        assert!(!path.exists());
    }
}
