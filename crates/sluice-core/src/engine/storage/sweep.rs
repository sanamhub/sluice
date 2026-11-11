//! The sweep of temporary folders a killed command left behind. A command that is killed cannot
//! remove anything, so the next command that makes one does it: a folder is removed when nobody
//! holds its lock and it is older than a day. A note a removal left for a folder is dropped at the
//! same age.

use std::{
    fs::{self, File},
    io,
    path::Path,
    time::SystemTime,
};

use super::{
    NOTE_PREFIX, remove_quietly, storage,
    temp::{PREFIX, STALE_AFTER},
};
use crate::{Error, Home};

/// Removes the temporary folders in the home's `tmp` that no running command holds and that are
/// older than [`STALE_AFTER`], and the notes of a removal that gave up on a folder as long ago. A
/// command calls this when it starts, so a killed one is cleaned up by the next command rather than
/// by a sweep of the output folder, which is never done by name.
pub fn sweep_temporary_store(now: SystemTime) -> Result<(), Error> {
    let swept = sweep(&Home::temporary_store()?, now);
    sweep_notes(Home::partial_records_for_read().as_deref(), now);
    swept
}

/// Removes the notes older than [`STALE_AFTER`] from the records folder.
///
/// A note is a claim on one folder, and a claim takes it the next time a receive needs that output.
/// A note that has stood for a day was not taken: the folder it names is refused as another
/// transfer's until the user removes it, which is what happened before a note stood for it, and a
/// note kept for longer would stand for a folder nobody is coming back for.
fn sweep_notes(dir: Option<&Path>, now: SystemTime) {
    let Some(dir) = dir else {
        return;
    };
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with(NOTE_PREFIX) {
            continue;
        }
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        let old = meta
            .modified()
            .ok()
            .and_then(|at| now.duration_since(at).ok())
            .is_some_and(|age| age >= STALE_AFTER);
        if old {
            let _ = fs::remove_file(entry.path());
        }
    }
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
    let lock = match open_lock(&path.join(super::temp::LOCK)) {
        Ok(lock) => lock,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return false,
        // A lock that cannot be read is not proof that the folder is free, and a folder removed
        // under a running command is worse than one an old command keeps.
        Err(_) => return true,
    };
    match lock.try_lock() {
        Ok(()) => {
            let _ = lock.unlock();
            false
        }
        Err(_) => true,
    }
}

/// Opens a lock file without following a link at its name. A link there would let the sweep lock,
/// and take a reading from, a file outside the folder, so the folder would be judged by something
/// that is not its lock.
fn open_lock(path: &Path) -> io::Result<File> {
    #[cfg(unix)]
    {
        use rustix::fs::{Mode, OFlags};

        let fd = rustix::fs::open(
            path,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )?;
        Ok(File::from(fd))
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;

        const OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        fs::OpenOptions::new()
            .read(true)
            .custom_flags(OPEN_REPARSE_POINT)
            .open(path)
    }
    #[cfg(not(any(unix, windows)))]
    {
        File::open(path)
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        time::{Duration, SystemTime},
    };

    use super::super::temp::temp_dir;
    use super::{PREFIX, STALE_AFTER, sweep};

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

    /// A note that has stood for a day is dropped, and a record beside it is not: the claim the note
    /// was waiting for did not come, and the folder it names is refused as another transfer's from
    /// then on, which is what happened before a note stood for it.
    #[test]
    fn the_sweep_drops_an_old_note_and_keeps_a_record() {
        use super::{NOTE_PREFIX, sweep_notes};

        let dir = tempfile::tempdir().unwrap();
        let note = dir.path().join(format!("{NOTE_PREFIX}notes"));
        fs::write(&note, "0".repeat(32)).unwrap();
        let record = dir.path().join("0123456789abcdef0123456789abcdef");
        fs::write(&record, "path /out/.sluice-partial-x\nroot one\n").unwrap();
        let later = SystemTime::now() + STALE_AFTER + Duration::from_secs(1);

        sweep_notes(Some(dir.path()), SystemTime::now());
        assert!(note.exists(), "a fresh note was swept");
        sweep_notes(Some(dir.path()), later);
        assert!(!note.exists(), "an old note was kept");
        assert!(record.exists(), "a record was swept as a note");
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

    /// The lock is opened without following a link. A link in its place would be read as the lock of
    /// another file, and a folder with no real lock would be taken for one nobody holds.
    #[cfg(unix)]
    #[test]
    fn a_link_in_place_of_the_lock_does_not_free_the_folder() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(format!("{PREFIX}linked"));
        fs::create_dir(&path).unwrap();
        let outside = dir.path().join("outside");
        std::os::unix::fs::symlink(&outside, path.join("lock")).unwrap();
        let later = SystemTime::now() + STALE_AFTER + Duration::from_secs(1);
        sweep(dir.path(), later).unwrap();
        assert!(
            path.exists(),
            "a folder whose lock is a link was swept as if nobody held it"
        );
    }
}
