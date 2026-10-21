//! Helpers for the tests that build a manifest from a real temporary folder.

use std::fs;
use std::path::Path;

use crate::manifest::{Build, Built, Finding, Kind, Manifest, Reason, Refusal};

pub(super) fn put(root: &Path, rel: &str, content: &[u8]) {
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create the parent folders");
    }
    fs::write(&path, content).expect("write the file");
}

pub(super) fn mkdir(root: &Path, rel: &str) {
    fs::create_dir_all(root.join(rel)).expect("create the folder");
}

pub(super) fn ready(root: &Path) -> Built {
    match Manifest::build(root).expect("the build runs") {
        Build::Ready(built) => built,
        Build::Refused(refusal) => panic!("refused: {:?}", refusal.problems),
    }
}

pub(super) fn refused(root: &Path) -> Refusal {
    match Manifest::build(root).expect("the build runs") {
        Build::Refused(refusal) => refusal,
        Build::Ready(built) => panic!("built {} entries", built.manifest.entries().len()),
    }
}

/// The path and kind of each entry, in manifest order.
pub(super) fn listing(built: &Built) -> Vec<(&str, Kind)> {
    built
        .manifest
        .entries()
        .iter()
        .map(|e| (e.path.as_str(), e.kind))
        .collect()
}

pub(super) fn finding(path: &str, reason: Reason) -> Finding {
    Finding {
        path: path.to_string(),
        reason,
    }
}

/// A symlink at `at` with the target text `target`. It returns `false` when the platform would not
/// make one, which on Windows is the case for a user without the privilege, and says so, so a
/// test that skips is a test whose skip shows in the output.
pub(super) fn symlink(target: &str, at: &Path) -> bool {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, at).expect("create the symlink");
        true
    }
    #[cfg(windows)]
    {
        match std::os::windows::fs::symlink_file(target, at) {
            Ok(()) => true,
            Err(e) => {
                eprintln!("SKIPPED: this account cannot create a symlink on Windows ({e})");
                false
            }
        }
    }
}
