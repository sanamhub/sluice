//! The folders of a checked tree, made before the files that go in them.

use std::{fs, path::PathBuf};

use super::super::storage;
use crate::{
    Error,
    manifest::{Kind, Manifest},
};

/// Every folder entry, in manifest order, and whether this transfer made it. Manifest order puts a
/// parent before its children, so one `create_dir` each is enough. A folder that is already there
/// belongs to this transfer when it lies under a top-level name this transfer made: the run that
/// was cut off created it, so its mode and time are still due.
pub(super) fn create_dirs(
    manifest: &Manifest,
    out: &std::path::Path,
    ours: &std::collections::BTreeSet<String>,
) -> Result<Vec<(PathBuf, u64, u32, bool)>, Error> {
    let mut dirs = Vec::new();
    for entry in manifest.entries() {
        if entry.kind != Kind::Dir {
            continue;
        }
        let target = entry.path.under(out);
        let made_here = match fs::create_dir(&target) {
            Ok(()) => true,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && target.is_dir() => {
                under_ours(entry.path.as_str(), ours)
            }
            Err(e) => return Err(storage::policy_path("create", &target, e)),
        };
        dirs.push((target, entry.mtime_ms, entry.mode, made_here));
    }
    Ok(dirs)
}

/// Whether `path` is inside a top-level name this transfer made.
fn under_ours(path: &str, ours: &std::collections::BTreeSet<String>) -> bool {
    ours.contains(path.split('/').next().unwrap_or(path))
}
