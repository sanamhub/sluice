//! The disk side of one receive: the space it needs, a private temporary folder, and the errors it
//! reports. The partial store itself is in [`store`], and installing a finished file in [`install`].

mod install;
mod meta;
mod open;
mod registry;
mod removals;
mod repair;
mod store;
mod temp;
mod temp_store;

pub use install::install;
pub use meta::MetaStore;
pub(crate) use open::hand_on;
pub use open::{Collector, Store};
#[cfg(test)]
pub(super) use open::{GC_INTERVAL, open_store_after};
#[cfg(test)]
pub(crate) use registry::remember_as;
pub use registry::{
    clash as registry_clash, discard_apart, find as find_record, forget as forget_record, with_hold,
};
pub use removals::wait_for_removals;
pub use repair::repair;
pub(crate) use repair::{changed, data_path, name_count};
pub use store::{FolderId, Owner, begin, identify, partial_dir, recorded};
pub use temp::{Temp, sweep_temporary_store, temp_dir, temp_path};
pub use temp_store::TempStore;

// A receive makes its store, records it and names it through `begin`. A test also drives those
// three steps on their own, so they are part of the test build and not of the library.
#[cfg(test)]
pub use registry::remember as remember_record;
#[cfg(test)]
pub use store::{MARKER, claim, remember};
#[cfg(test)]
pub use temp::PREFIX as TEMP_PREFIX;

use std::{fs, io, path::Path, time::Duration};

use crate::Error;

/// The waits before another try at a name another process holds: a folder that a removal cannot
/// take, or a store folder that the repair cannot open while a scanner has it. They add up to under
/// two seconds together.
const HOLD_DELAYS: [u64; 7] = [10, 20, 40, 80, 160, 320, 640];

/// How much room a receive of `content` bytes needs before it starts. The store keeps the partial
/// blob and its outboard, about 0.4% of the content, and the manifest and metadata are small. A
/// finished file is moved out of the store rather than copied, so with hard links this is a peak and
/// not 2x. A volume that cannot link copies each file out while its blob is still in the store, so
/// there the whole content is held twice.
pub fn needed_bytes(content: u64, links: bool) -> u64 {
    const OUTBOARD_PARTS_PER_THOUSAND: u64 = 8;
    const SLACK: u64 = 1 << 20;
    let room = content.saturating_add(content / 1000 * OUTBOARD_PARTS_PER_THOUSAND);
    if links {
        room.saturating_add(SLACK)
    } else {
        room.saturating_mul(2).saturating_add(SLACK)
    }
}

/// Bytes free to this user on the volume that holds `path`.
pub fn available_bytes(path: &Path) -> Result<u64, Error> {
    imp::available(path).map_err(|e| {
        Error::Storage(format!(
            "cannot read the free space at {}: {e}",
            path.display()
        ))
    })
}

/// Whether the volume holding `dir` can make a hard link, which decides whether a finished file can
/// be moved out of the store or has to be copied out beside it. Probed rather than assumed: a
/// transfer that assumes links on a volume without them can fill the disk with the copy.
pub fn supports_links(dir: &Path) -> bool {
    let Ok(tmp) = temp_dir(dir) else {
        return false;
    };
    let from = temp_path(tmp.path(), 0);
    let to = temp_path(tmp.path(), 1);
    fs::write(&from, b"").is_ok() && fs::hard_link(&from, &to).is_ok()
}

/// Refuses when `dir` does not have room for a transfer of `content` bytes plus the store beside
/// it. The store is what a receive writes before it writes the file, so this is the volume that
/// has to hold both during the transfer.
pub fn ensure_space(dir: &Path, content: u64) -> Result<(), Error> {
    let available = available_bytes(dir)?;
    let needed = needed_bytes(content, true);
    if available < needed {
        return Err(Error::Storage(format!(
            "{} has {available} bytes free and this transfer needs about {needed}",
            dir.display()
        )));
    }
    Ok(())
}

/// The one clash message, naming the first path and both ways out.
pub fn clash(target: &Path) -> Error {
    Error::Policy(format!(
        "{} already exists; pass --force to replace it, or choose another --out",
        target.display()
    ))
}

pub fn storage(doing: &str, path: &Path, source: io::Error) -> Error {
    Error::Storage(format!("cannot {doing} {}: {source}", path.display()))
}

pub fn policy_path(doing: &str, path: &Path, source: io::Error) -> Error {
    Error::Policy(format!("cannot {doing} {}: {source}", path.display()))
}

/// A removal a test can hold at its start, keyed by folder, so the test can act while the removal
/// is still to be tried. A test run shares the one list with every other removal in the process, so
/// an arm is taken by the first removal of its own folder, and only once; several tests can arm at
/// the same time because no two of them use one folder.
#[cfg(test)]
pub(crate) mod parked {
    use std::{
        path::{Path, PathBuf},
        sync::{Mutex, mpsc},
    };

    type Arm = (PathBuf, mpsc::Sender<()>, mpsc::Receiver<()>);

    static ARMED: Mutex<Vec<Arm>> = Mutex::new(Vec::new());

    /// Holds the next removal of `path` until the returned sender is signalled, and reports on the
    /// returned receiver when the removal has reached this point.
    pub(crate) fn arm(path: &Path) -> (mpsc::Receiver<()>, mpsc::Sender<()>) {
        let (started, started_at) = mpsc::channel();
        let (release, released_at) = mpsc::channel();
        let mut armed = ARMED.lock().unwrap();
        armed.push((path.to_owned(), started, released_at));
        (started_at, release)
    }

    /// Called at the start of every removal. Blocks only the removal of an armed folder.
    pub(crate) fn wait(path: &Path) {
        let armed = {
            let mut armed = ARMED.lock().unwrap();
            armed
                .iter()
                .position(|(at, _, _)| at == path)
                .map(|index| armed.swap_remove(index))
        };
        if let Some((_, started, release)) = armed {
            let _ = started.send(());
            let _ = release.recv();
        }
    }
}

/// Removes a file or folder that this receive made, ignoring a failure: the transfer itself
/// succeeded or failed for a reason worth reporting first.
///
/// A store's database can still be closing when the folder above it is removed, and a scanner on
/// Windows can hold a file for a moment after that, so a failure that may clear on its own is tried
/// again for a short while. A folder that still cannot be removed is left to the sweep, which
/// removes a temporary folder nobody holds; failing a send over it would hide the outcome that
/// matters. The removal is counted while it runs, and a path it could not remove is kept, so a
/// command that is ending waits for it and tries it again ([`wait_for_removals`]).
pub fn remove_quietly(path: &Path) {
    let _doing = removals::Doing::begin();
    #[cfg(test)]
    parked::wait(path);
    if quiet_attempts(path) {
        removals::gone(path);
    } else {
        removals::left(path);
    }
}

/// One attempt at the removal, then the waits for a failure that a moment can clear, and whether
/// the path is gone.
fn quiet_attempts(path: &Path) -> bool {
    let mut last = match remove_once(path) {
        Ok(()) => return true,
        Err(error) => error,
    };
    for delay in HOLD_DELAYS {
        if !retryable(&last) {
            return false;
        }
        std::thread::sleep(Duration::from_millis(delay));
        match remove_once(path) {
            Ok(()) => return true,
            Err(error) => last = error,
        }
    }
    false
}

/// Removes `path` once and says whether it is gone. A temporary folder can be a folder or a file,
/// so both names are tried, and a name that is already gone counts as removed.
pub(super) fn remove_once(path: &Path) -> io::Result<()> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(second) if second.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(error),
        },
    }
}

/// Whether a removal failed for a reason that a moment can clear: a file another process still
/// holds, or a folder whose last entry went away between the listing and the removal. The Unix
/// codes are named through `rustix` because they differ between the Unixes: `ENOTEMPTY` is 39 on
/// Linux and 66 on macOS.
fn retryable(error: &io::Error) -> bool {
    #[cfg(windows)]
    if matches!(error.raw_os_error(), Some(5 | 32 | 33 | 145)) {
        return true;
    }
    #[cfg(unix)]
    if let Some(code) = error.raw_os_error() {
        return code == rustix::io::Errno::BUSY.raw_os_error()
            || code == rustix::io::Errno::NOTEMPTY.raw_os_error();
    }
    error.kind() == io::ErrorKind::WouldBlock
}

#[cfg(unix)]
mod imp {
    use std::path::Path;

    pub(super) fn available(path: &Path) -> Result<u64, std::io::Error> {
        let stat = rustix::fs::statvfs(path)?;
        Ok(stat.f_bavail.saturating_mul(stat.f_frsize))
    }
}

#[cfg(windows)]
mod imp {
    use std::path::Path;

    pub(super) fn available(path: &Path) -> Result<u64, std::io::Error> {
        crate::home::private_windows::free_bytes(path)
    }
}

#[cfg(not(any(unix, windows)))]
mod imp {
    use std::path::Path;

    pub(super) fn available(_path: &Path) -> Result<u64, std::io::Error> {
        Err(std::io::Error::other("this platform has no call for it"))
    }
}

#[cfg(test)]
mod tests;
