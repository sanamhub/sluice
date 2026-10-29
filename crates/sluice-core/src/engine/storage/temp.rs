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
    sync::{Arc, Condvar, Mutex},
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

/// A temporary folder whose lock could not be taken is still in use, so its folder time is touched
/// while it lives. The sweep goes by that time; without the touch a transfer that runs past
/// [`STALE_AFTER`] on a file system whose lock failed for a moment would be removed under it.
const TOUCH_EVERY: Duration = Duration::from_secs(60 * 60);

/// A temporary folder for one store, with the lock that keeps the sweep away from it. The folder is
/// removed and the lock released when the value is dropped.
pub struct Temp {
    path: PathBuf,
    /// Held for as long as this value lives.
    _lock: File,
    /// Started only when the lock could not be taken, so the folder still looks in use.
    heartbeat: Option<Heartbeat>,
    /// Set by an explicit removal, so the drop that follows is not a second attempt.
    gone: bool,
}

/// A thread that keeps a temporary folder's time fresh. It stops when its `Temp` is dropped.
struct Heartbeat {
    stopped: Arc<(Mutex<bool>, Condvar)>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Heartbeat {
    fn start(path: PathBuf, every: Duration) -> Self {
        let stopped = Arc::new((Mutex::new(false), Condvar::new()));
        let flag = Arc::clone(&stopped);
        let thread = std::thread::Builder::new()
            .name("sluice-temp-touch".to_string())
            .spawn(move || {
                let (lock, condvar) = &*flag;
                let mut stopped = lock.lock().unwrap_or_else(|p| p.into_inner());
                loop {
                    let (guard, _) = condvar
                        .wait_timeout(stopped, every)
                        .unwrap_or_else(|p| p.into_inner());
                    stopped = guard;
                    if *stopped {
                        return;
                    }
                    drop(stopped);
                    touch(&path);
                    stopped = lock.lock().unwrap_or_else(|p| p.into_inner());
                }
            })
            .ok();
        Self { stopped, thread }
    }
}

impl Drop for Heartbeat {
    fn drop(&mut self) {
        let (lock, condvar) = &*self.stopped;
        *lock.lock().unwrap_or_else(|p| p.into_inner()) = true;
        condvar.notify_all();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Adds and removes a name inside `path`, which sets the folder's time on every platform.
fn touch(path: &Path) {
    let marker = path.join(".in-use");
    if fs::write(&marker, b"").is_ok() {
        let _ = fs::remove_file(&marker);
    }
}

impl Temp {
    /// The folder itself.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Removes the folder on a blocking thread and waits for it to be gone. A removal can wait for
    /// a file another process still holds; those waits belong on a thread that may block, and a
    /// caller that promises the folder is gone when it returns waits for this.
    pub(crate) async fn remove_now(mut self) {
        self.heartbeat.take();
        let path = self.path.clone();
        let _ = tokio::task::spawn_blocking(move || remove_quietly(&path)).await;
        self.gone = true;
    }

    /// Removes the folder on a blocking thread without waiting. A process that ends before the
    /// removal does leaves the folder to the sweep of temporary folders.
    pub(crate) fn remove_apart(mut self) {
        self.heartbeat.take();
        self.gone = true;
        let path = self.path.clone();
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                handle.spawn_blocking(move || remove_quietly(&path));
            }
            Err(_) => remove_quietly(&path),
        }
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        if self.gone {
            return;
        }
        // Stopped first, so a touch cannot recreate a name in a folder that is being removed.
        self.heartbeat.take();
        // A drop can run on an async thread, where the waits a held file needs would stop other
        // work. One attempt is immediate and almost always enough; the waits go to a thread of
        // their own, and a process that ends first leaves the folder to the sweep.
        if tokio::runtime::Handle::try_current().is_err() {
            remove_quietly(&self.path);
            return;
        }
        if super::remove_once(&self.path).is_ok() {
            return;
        }
        let path = std::mem::take(&mut self.path);
        let _ = std::thread::Builder::new()
            .name("sluice-temp-remove".to_string())
            .spawn(move || remove_quietly(&path));
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
    // Left unlocked on purpose rather than failing: see above. The sweep then decides by the
    // folder's time, so the heartbeat keeps it fresh while this command runs.
    let heartbeat = match lock.try_lock() {
        Ok(()) => None,
        Err(_) => Some(Heartbeat::start(path.clone(), TOUCH_EVERY)),
    };
    Ok(Temp {
        path,
        _lock: lock,
        heartbeat,
        gone: false,
    })
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
    let lock = match open_lock(&path.join(LOCK)) {
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

    /// A folder whose lock could not be taken is still in use, and the sweep would take it for one a
    /// killed command left once a day had passed. The heartbeat keeps the folder's time fresh, which
    /// is the same time the sweep reads.
    #[test]
    fn a_folder_whose_lock_could_not_be_taken_is_touched_while_it_lives() {
        use super::Heartbeat;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("folder");
        fs::create_dir(&path).unwrap();
        let before = fs::metadata(&path).unwrap().modified().unwrap();
        let beat = Heartbeat::start(path.clone(), Duration::from_millis(20));
        let mut touched = false;
        for _ in 0..100 {
            std::thread::sleep(Duration::from_millis(10));
            if fs::metadata(&path).unwrap().modified().unwrap() > before {
                touched = true;
                break;
            }
        }
        drop(beat);
        assert!(
            touched,
            "a long transfer's folder was left to look abandoned"
        );
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
