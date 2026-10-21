//! The one-writer lock of a home.

use std::fs::{self, File, OpenOptions, TryLockError};
use std::path::Path;

use super::storage;
use crate::Error;

/// Takes the exclusive lock on `dir/lock` and records this process's id in `dir/lock.pid`. The
/// lock lives as long as the returned file does.
///
/// The id is in a second file because Windows locks a file against every read by another handle,
/// so a process that was refused could not read the id out of the file that is locked. The
/// operating system drops the lock when the process ends, however it ends, so a lock left by a
/// process that died needs no cleaning: both files are still there and nobody holds the lock.
pub(super) fn take(dir: &Path) -> Result<File, Error> {
    let path = dir.join("lock");
    // The file has no content. It exists to be locked.
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .map_err(|e| storage("open", &path, e))?;
    match file.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => return Err(held(dir)),
        Err(TryLockError::Error(e)) => return Err(storage("lock", &path, e)),
    }
    // After the lock, so the id in the file is the holder's and a refused process never changes it.
    let pid_file = dir.join("lock.pid");
    fs::write(&pid_file, format!("{}\n", std::process::id()))
        .map_err(|e| storage("write the process id into", &pid_file, e))?;
    Ok(file)
}

/// The error for a lock another process holds, naming that process when its id can be read. The id
/// is that of whoever held the lock last, so in the instant between a process taking the lock and
/// writing its id, it can be the one before. That makes the name a hint and never a check.
fn held(dir: &Path) -> Error {
    let pid = fs::read_to_string(dir.join("lock.pid"))
        .ok()
        .and_then(|text| text.trim().parse::<u32>().ok());
    let who = match pid {
        Some(pid) => format!("another sluice process (pid {pid})"),
        None => "another sluice process".to_string(),
    };
    Error::Policy(format!("{who} is using the store at {}", dir.display()))
}
