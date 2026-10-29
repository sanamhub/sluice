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

/// The text a manifest carries for `name`, or [`Reason::NotUtf8`] when the file system holds a
/// name that UTF-8 cannot. Split from [`Walker::path_of`] so the rule is checked without a file
/// system, which APFS cannot build for such a name.
pub(super) fn manifest_name(name: &OsStr) -> Result<&str, Reason> {
    name.to_str().ok_or(Reason::NotUtf8)
}

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

/// Lists the arguments of one `send`. Each argument contributes its own name as a top-level entry,
/// so `send DIR` writes `DIR/...` in the output and `send a.mov photos/` writes `a.mov` and
/// `photos/...`. With `name_roots` false the folder is treated as a tree whose own name the caller
/// already carries, which is what [`crate::manifest::Manifest::build`] does.
pub(super) fn walk_paths(paths: &[PathBuf], name_roots: bool) -> Result<Walk, Error> {
    if paths.is_empty() {
        return Err(Error::Policy("there is nothing to send".to_string()));
    }
    let mut walker = Walker::default();
    let mut names: Vec<(String, String)> = Vec::new();
    for path in paths {
        add_argument(&mut walker, &mut names, path, name_roots)?;
    }
    while let Some(folder) = walker.stack.pop() {
        walker.read_folder(&folder);
    }
    Ok(walker.out)
}

/// One argument of `send`: a file is a top-level entry of its own, a folder is walked with its
/// name as the top-level entry when `name_roots` is set.
fn add_argument(
    walker: &mut Walker,
    names: &mut Vec<(String, String)>,
    path: &Path,
    name_roots: bool,
) -> Result<(), Error> {
    let shown = path.display().to_string();
    let before = fs::symlink_metadata(path)
        .map_err(|e| Error::Storage(format!("cannot read the path to send: {}", e.kind())))?;
    // `metadata` follows a link. A folder may be a link, a file may not: sending a file link
    // would send a name whose content lives somewhere else.
    let meta = fs::metadata(path)
        .map_err(|e| Error::Storage(format!("cannot read the path to send: {}", e.kind())))?;
    let Some(name) = path.file_name().and_then(OsStr::to_str) else {
        return Err(Error::Policy(format!("{shown} has no UTF-8 name")));
    };
    let name = unique_name(names, nfc_name(name)?, &shown)?;
    if meta.is_dir() {
        let prefix = if name_roots {
            walker.root_dir(&name, path, &meta);
            name.clone()
        } else {
            String::new()
        };
        walker.stack.push(Folder {
            disk: path.to_path_buf(),
            shown: if name_roots { name } else { String::new() },
            path: Some(prefix),
        });
    } else {
        walker
            .out
            .items
            .push(file_item(&shown, &name, &before, path)?);
    }
    Ok(())
}

/// The top-level name, refused when another argument already carries it.
fn unique_name(
    names: &mut Vec<(String, String)>,
    name: String,
    shown: &str,
) -> Result<String, Error> {
    if let Some((_, other)) = names.iter().find(|(existing, _)| existing == &name) {
        return Err(Error::Policy(format!(
            "{other} and {shown} would both arrive as {name}; rename one of them or send \
             them in separate transfers"
        )));
    }
    names.push((name.clone(), shown.to_string()));
    Ok(name)
}

/// One file argument as a top-level entry. A link is refused: its content lives somewhere else.
fn file_item(shown: &str, name: &str, before: &fs::Metadata, path: &Path) -> Result<Item, Error> {
    if before.file_type().is_symlink() {
        return Err(Error::Policy(format!(
            "{shown} is a symlink; send the file it points at by its real path"
        )));
    }
    if !before.is_file() {
        return Err(Error::Policy(format!(
            "{shown} is not a regular file or a folder"
        )));
    }
    let entry_path = ManifestPath::parse_rule(name).map_err(|rule| {
        Error::Policy(format!(
            "{shown} has a name a receiver would reject (rule {rule})"
        ))
    })?;
    Ok(Item {
        path: entry_path,
        shown: name.to_string(),
        what: What::File {
            disk: path.to_path_buf(),
        },
    })
}

/// The name as the manifest carries it, after the checks a receiver applies to every component.
fn nfc_name(name: &str) -> Result<String, Error> {
    let nfc: String = name.nfc().collect();
    match component_rule(&nfc) {
        None => Ok(nfc),
        Some(rule) => Err(Error::Policy(format!(
            "a path to send has a name a receiver would reject (rule {rule})"
        ))),
    }
}

#[derive(Default)]
struct Walker {
    stack: Vec<Folder>,
    out: Walk,
}

impl Walker {
    /// The folder an argument names, as its top-level entry. A caller that is given a tree whose
    /// own name it already carries passes `name_roots` false and gets no entry here.
    fn root_dir(&mut self, name: &str, disk: &Path, meta: &fs::Metadata) {
        // `nfc_name` checked the component, so the path rule cannot fail here.
        let path = ManifestPath::parse_rule(name).expect("a checked name");
        let what = What::Dir {
            mtime_ms: meta::mtime_ms(meta),
            mode: meta::mode(meta),
        };
        self.out.items.push(Item {
            path,
            shown: disk.display().to_string(),
            what,
        });
    }

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
        let text = match manifest_name(name) {
            Ok(text) => text,
            Err(reason) => {
                self.problem(shown.to_string(), reason);
                return None;
            }
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
