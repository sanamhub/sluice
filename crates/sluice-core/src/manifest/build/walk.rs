//! Reading the folder: every name under it, classified, with an explicit list of folders left to
//! read so a deep tree cannot overflow the stack.
//!
//! Nothing is hashed here. A folder with a bad name is refused after this walk and before any
//! file is read, so a user who has to rename something does not wait for a 100 GB hash first.

use std::ffi::OsStr;
use std::fs::{self, DirEntry, FileType};
use std::path::{Path, PathBuf};

use unicode_normalization::UnicodeNormalization;

use super::{Finding, Reason, meta};
use crate::Error;
use crate::manifest::link::LinkTarget;
use crate::manifest::path::{ManifestPath, component_rule};

/// What the walk found that goes into the manifest, in no order.
pub(super) struct Item {
    pub path: ManifestPath,
    /// The path as the sender sees it, for a message that has to name the file.
    pub shown: String,
    pub what: What,
}

pub(super) enum What {
    Dir {
        mtime_ms: u64,
        mode: u32,
    },
    /// Hashed after the walk. Its size, time and mode come from that read.
    File {
        disk: PathBuf,
    },
    Symlink {
        target: LinkTarget,
        mtime_ms: u64,
    },
}

#[derive(Default)]
pub(super) struct Walk {
    pub items: Vec<Item>,
    /// Names a receiver would reject, and files that could not be read.
    pub problems: Vec<Finding>,
    /// Symlinks and special files that are left out.
    pub skipped: Vec<Finding>,
}

/// A folder still to be read.
struct Folder {
    disk: PathBuf,
    shown: String,
    /// Its path in the manifest, empty for the folder that was sent. `None` when the folder has a
    /// name a receiver would reject, so nothing below it has a path to be listed under.
    path: Option<String>,
}

/// Lists everything under `root`, which is followed if it is a symlink and is the only link that
/// is.
pub(super) fn walk(root: &Path) -> Result<Walk, Error> {
    // `metadata` follows a link, and that is the one link the walk follows.
    let meta = fs::metadata(root)
        .map_err(|e| Error::Storage(format!("cannot read the folder to send: {}", e.kind())))?;
    if !meta.is_dir() {
        return Err(Error::Policy(
            "the path to send is not a folder".to_string(),
        ));
    }
    let mut walker = Walker::default();
    walker.stack.push(Folder {
        disk: root.to_path_buf(),
        shown: String::new(),
        path: Some(String::new()),
    });
    while let Some(folder) = walker.stack.pop() {
        walker.read_folder(&folder);
    }
    Ok(walker.out)
}

#[derive(Default)]
struct Walker {
    stack: Vec<Folder>,
    out: Walk,
}

impl Walker {
    fn read_folder(&mut self, folder: &Folder) {
        // The folder that was sent has no name of its own, and a list shown to the user would
        // have a blank line for it.
        let shown = if folder.shown.is_empty() {
            "."
        } else {
            &folder.shown
        };
        let entries = match fs::read_dir(&folder.disk) {
            Ok(entries) => entries,
            Err(e) => return self.problem(shown.to_string(), Reason::Unreadable(e.kind())),
        };
        for entry in entries {
            match entry {
                Ok(entry) => self.visit(folder, &entry),
                Err(e) => self.problem(shown.to_string(), Reason::Unreadable(e.kind())),
            }
        }
    }

    fn problem(&mut self, path: String, reason: Reason) {
        self.out.problems.push(Finding { path, reason });
    }

    fn skip(&mut self, path: String, reason: Reason) {
        self.out.skipped.push(Finding { path, reason });
    }

    fn visit(&mut self, folder: &Folder, entry: &DirEntry) {
        let name = entry.file_name();
        let shown = join(&folder.shown, &name.to_string_lossy());
        // `file_type` does not follow a symlink, which is how a link is told from its target.
        let kind = match entry.file_type() {
            Ok(kind) => kind,
            Err(e) => return self.problem(shown, Reason::Unreadable(e.kind())),
        };
        let path = self.path_of(folder, &name, &shown);
        let Some(path) = path else {
            // The folder is still read, so a fault inside it is listed in this run, not the next.
            if kind.is_dir() {
                self.stack.push(Folder {
                    disk: entry.path(),
                    shown,
                    path: None,
                });
            }
            return;
        };
        self.add(entry, kind, path, shown);
    }

    /// The manifest path of an entry, or `None` after listing why it has none. The name is
    /// normalized to NFC here, because a receiver does not normalize and the manifest has to
    /// carry the form it accepts.
    fn path_of(&mut self, folder: &Folder, name: &OsStr, shown: &str) -> Option<ManifestPath> {
        let Some(text) = name.to_str() else {
            self.problem(shown.to_string(), Reason::NotUtf8);
            return None;
        };
        let nfc: String = text.nfc().collect();
        if let Some(rule) = component_rule(&nfc) {
            self.problem(shown.to_string(), Reason::PathRule(rule));
            return None;
        }
        // A folder that was listed already carries the fault for everything below it.
        let parent = folder.path.as_deref()?;
        let whole = if parent.is_empty() {
            nfc
        } else {
            format!("{parent}/{nfc}")
        };
        match ManifestPath::parse_rule(&whole) {
            Ok(path) => Some(path),
            Err(rule) => {
                self.problem(shown.to_string(), Reason::PathRule(rule));
                None
            }
        }
    }

    fn add(&mut self, entry: &DirEntry, kind: FileType, path: ManifestPath, shown: String) {
        if kind.is_symlink() {
            return self.link(entry, path, shown);
        }
        if kind.is_dir() {
            // Not `DirEntry::metadata`: on Windows that is what the listing cached, and the time
            // of a folder that was just written to is stale in it.
            let meta = match fs::symlink_metadata(entry.path()) {
                Ok(meta) => meta,
                Err(e) => return self.problem(shown, Reason::Unreadable(e.kind())),
            };
            self.stack.push(Folder {
                disk: entry.path(),
                shown: shown.clone(),
                path: Some(path.as_str().to_owned()),
            });
            let what = What::Dir {
                mtime_ms: meta::mtime_ms(&meta),
                mode: meta::mode(&meta),
            };
            return self.out.items.push(Item { path, shown, what });
        }
        if kind.is_file() {
            let what = What::File { disk: entry.path() };
            return self.out.items.push(Item { path, shown, what });
        }
        self.skip(shown, Reason::NotAFileOrFolder);
    }

    /// A symlink is recorded with its target as written and never followed. One whose target a
    /// receiver would reject is left out, because that target means nothing on another machine.
    fn link(&mut self, entry: &DirEntry, path: ManifestPath, shown: String) {
        let target = match fs::read_link(entry.path()) {
            Ok(target) => target,
            Err(e) => return self.problem(shown, Reason::Unreadable(e.kind())),
        };
        let Some(text) = target.to_str() else {
            return self.skip(shown, Reason::LinkTargetNotUtf8);
        };
        let target = match LinkTarget::parse_rule(&portable(text, cfg!(windows))) {
            Ok(target) if target.stays_inside(&path) => target,
            Ok(_) => return self.skip(shown, Reason::LinkTarget(5)),
            Err(rule) => return self.skip(shown, Reason::LinkTarget(rule)),
        };
        let mtime_ms = fs::symlink_metadata(entry.path()).map_or(0, |meta| meta::mtime_ms(&meta));
        self.out.items.push(Item {
            path,
            shown,
            what: What::Symlink { target, mtime_ms },
        });
    }
}

fn join(folder: &str, name: &str) -> String {
    if folder.is_empty() {
        name.to_string()
    } else {
        format!("{folder}/{name}")
    }
}

/// A Windows link points with `\`, which is its separator, so it is the `/` of a manifest. On
/// Unix `\` is a character of a name, and a target that holds one is left for rule 4 to refuse.
/// The platform is an argument so that both cases are tested on any machine, and a Windows one
/// needs no symlink privilege for it.
pub(super) fn portable(target: &str, windows: bool) -> String {
    if windows {
        target.replace('\\', "/")
    } else {
        target.to_string()
    }
}
