//! The confined walk, which a daemon always uses. A caller opens and proves its folders itself and
//! hands over their handles; every listing and every open here is relative to a handle, never
//! following a link, so a folder or an ancestor swapped for a link while the walk runs cannot lead
//! out of the folders that were handed over. A file is found again, to be hashed and to be copied,
//! the same way, and is accepted only if it is the object the walk saw there.
//!
//! The handles are `dirs::Dir`: on Unix every open is an `openat` with `O_NOFOLLOW` from the folder's
//! descriptor, and on Windows an `NtCreateFile` with the folder as its root directory.

use std::ffi::OsStr;
use std::fs::File;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;

use unicode_normalization::UnicodeNormalization;

use super::argument::{nfc_name, unique_name};
use super::dirs::{Dir, Kind};
use super::handles::{Below, ConfinedArg, open_dir_below, open_in};
use super::walk::{Item, Walk, What};
use super::{Finding, Reason, meta};
use crate::Error;
use crate::engine::{ObjectId, facts, is_link};
use crate::manifest::path::{ManifestPath, component_rule};

fn refused(shown: &str, why: &str) -> Error {
    Error::Policy(format!(
        "{} is refused: {why}",
        crate::manifest::path::shown(shown)
    ))
}

#[cfg(test)]
type Hook = Box<dyn Fn(&str)>;

#[cfg(test)]
thread_local! {
    static BEFORE_LISTING: std::cell::RefCell<Hook> = std::cell::RefCell::new(Box::new(|_| {}));
}

/// Runs `hook` with the name of each folder just before the walk opens it to list it, which is where
/// a test swaps a folder for a link, the one moment a real run cannot be made to hit on purpose.
#[cfg(test)]
pub(super) fn before_listing(hook: impl Fn(&str) + 'static) {
    BEFORE_LISTING.with(|slot| *slot.borrow_mut() = Box::new(hook));
}

#[cfg(test)]
thread_local! {
    static AFTER_OPEN: std::cell::RefCell<Hook> = std::cell::RefCell::new(Box::new(|_| {}));
    static LISTED: std::cell::RefCell<Vec<(String, Vec<String>)>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// Runs `hook` with the name of each folder after the walk opened it and before it lists what the
/// open handle holds, where a test swaps the folder the handle was opened from.
#[cfg(test)]
pub(super) fn after_open(hook: impl Fn(&str) + 'static) {
    AFTER_OPEN.with(|slot| *slot.borrow_mut() = Box::new(hook));
}

/// The folders listed so far and the names each gave, then forgets them.
#[cfg(test)]
pub(super) fn take_listed() -> Vec<(String, Vec<String>)> {
    LISTED.with(|slot| std::mem::take(&mut *slot.borrow_mut()))
}

/// A folder still to be read.
struct Pending {
    root: Arc<Dir>,
    volume: u64,
    rel: PathBuf,
    shown: String,
    /// Its path in the manifest, or `None` when it or an ancestor has a name a receiver rejects.
    path: Option<String>,
}

/// What judging one open handle found.
struct Proved {
    meta: std::fs::Metadata,
    id: ObjectId,
    links: u64,
}

/// Reads an open handle's metadata and identity, and refuses a link or an object on another
/// volume than `volume` (zero for no check).
fn prove(file: &File, shown: &str, volume: Option<u64>) -> Result<Proved, Error> {
    let unreadable = |e: io::Error| refused(shown, &format!("it cannot be read ({})", e.kind()));
    if is_link(file).map_err(unreadable)? {
        return Err(refused(shown, "it is a link, which a daemon never sends"));
    }
    let meta = file.metadata().map_err(unreadable)?;
    let found = facts(file).map_err(unreadable)?;
    if volume.is_some_and(|volume| found.identity.volume() != volume) {
        return Err(refused(
            shown,
            "it is on another volume than the folder sent",
        ));
    }
    Ok(Proved {
        meta,
        id: found.identity,
        links: found.links,
    })
}

/// Lists the arguments, each as its own top-level entry, the way `walk_paths` does.
pub(super) fn walk(args: Vec<ConfinedArg>) -> Result<Walk, Error> {
    if args.is_empty() {
        return Err(Error::Policy("there is nothing to send".to_string()));
    }
    let mut out = Walk::default();
    let mut stack: Vec<Pending> = Vec::new();
    let mut names: Vec<(String, String)> = Vec::new();
    for arg in args {
        let shown = arg.name.clone();
        let name = unique_name(&mut names, nfc_name(&arg.name)?, &shown)?;
        let root = Arc::new(Dir::from_file(arg.handle));
        if arg.file {
            file_argument(&mut out, root, &arg.name, name, arg.expect)?;
        } else {
            let handle = root.try_clone().map_err(|e| unreadable(&shown, &e))?;
            folder_argument(&mut out, &mut stack, root, &handle, name, arg.expect)?;
        }
    }
    while let Some(folder) = stack.pop() {
        read_folder(&mut out, &mut stack, &folder)?;
    }
    Ok(out)
}

fn unreadable(shown: &str, error: &io::Error) -> Error {
    refused(shown, &format!("it cannot be read ({})", error.kind()))
}

/// A file argument. It is opened by the spelling the folder has, `raw`, and not by the NFC form the
/// manifest carries, since a folder can hold both spellings as two files.
fn file_argument(
    out: &mut Walk,
    parent: Arc<Dir>,
    raw: &str,
    name: String,
    expect: Option<ObjectId>,
) -> Result<(), Error> {
    let file = open_in(&parent, OsStr::new(raw)).map_err(|e| unreadable(raw, &e))?;
    let seen = prove(&file, raw, None)?;
    if !seen.meta.is_file() {
        return Err(refused(raw, "it is not a regular file or a folder"));
    }
    if seen.links > 1 {
        return Err(refused(raw, MORE_THAN_ONE_NAME));
    }
    if expect.is_some_and(|expected| expected != seen.id) {
        return Err(refused(raw, "it is not the object that was checked"));
    }
    let path = ManifestPath::parse_rule(&name).map_err(|rule| {
        Error::Policy(format!(
            "{name} has a name a receiver would reject (rule {rule})"
        ))
    })?;
    out.items.push(Item {
        path,
        shown: name.clone(),
        what: What::File {
            disk: PathBuf::from(raw),
            below: Some(Below {
                root: parent,
                rel: PathBuf::from(raw),
                id: seen.id,
            }),
        },
    });
    Ok(())
}

const MORE_THAN_ONE_NAME: &str = "it has more than one name, and another may lie outside the root";

fn folder_argument(
    out: &mut Walk,
    stack: &mut Vec<Pending>,
    root: Arc<Dir>,
    handle: &Dir,
    name: String,
    expect: Option<ObjectId>,
) -> Result<(), Error> {
    let file = handle
        .try_clone()
        .map_err(|e| unreadable(&name, &e))?
        .into_file();
    let seen = prove(&file, &name, None)?;
    if expect.is_some_and(|expected| expected != seen.id) {
        return Err(refused(&name, "it is not the object that was checked"));
    }
    let path = ManifestPath::parse_rule(&name).map_err(|rule| {
        Error::Policy(format!(
            "{name} has a name a receiver would reject (rule {rule})"
        ))
    })?;
    out.items.push(Item {
        path,
        shown: name.clone(),
        what: What::Dir {
            mtime_ms: meta::mtime_ms(&seen.meta),
            mode: meta::mode(&seen.meta),
        },
    });
    stack.push(Pending {
        root,
        volume: seen.id.volume(),
        rel: PathBuf::new(),
        shown: name.clone(),
        path: Some(name),
    });
    Ok(())
}

/// The manifest path of an entry, or the reason it has none.
fn manifest_path(parent: &str, name: &OsStr) -> Result<ManifestPath, Reason> {
    let text = name.to_str().ok_or(Reason::NotUtf8)?;
    let nfc: String = text.nfc().collect();
    if let Some(rule) = component_rule(&nfc) {
        return Err(Reason::PathRule(rule));
    }
    let whole = format!("{parent}/{nfc}");
    ManifestPath::parse_rule(&whole).map_err(Reason::PathRule)
}

fn read_folder(out: &mut Walk, stack: &mut Vec<Pending>, folder: &Pending) -> Result<(), Error> {
    #[cfg(test)]
    BEFORE_LISTING.with(|hook| (hook.borrow())(&folder.shown));
    let dir = open_dir_below(&folder.root, &folder.rel).map_err(|e| {
        refused(
            &folder.shown,
            &format!("it cannot be opened ({})", e.kind()),
        )
    })?;
    #[cfg(test)]
    AFTER_OPEN.with(|hook| (hook.borrow())(&folder.shown));
    let names = dir.names().map_err(|e| unreadable(&folder.shown, &e))?;
    #[cfg(test)]
    LISTED.with(|slot| {
        let listed = names
            .iter()
            .map(|n| n.to_string_lossy().into_owned())
            .collect();
        slot.borrow_mut().push((folder.shown.clone(), listed));
    });
    for name in names {
        let shown = format!("{}/{}", folder.shown, name.to_string_lossy());
        let kind = dir.kind(&name).map_err(|e| unreadable(&shown, &e))?;
        let path = match folder.path.as_deref() {
            Some(parent) => match manifest_path(parent, &name) {
                Ok(path) => Some(path),
                Err(reason) => {
                    out.problems.push(Finding {
                        path: shown.clone(),
                        reason,
                    });
                    None
                }
            },
            None => None,
        };
        if kind == Kind::Link {
            return Err(refused(&shown, "it is a link, which a daemon never sends"));
        }
        if kind == Kind::Dir {
            read_subfolder(out, stack, folder, &dir, &name, &shown, path)?;
        } else if kind == Kind::File {
            if let Some(path) = path {
                add_file(out, folder, &dir, &name, shown, path)?;
            }
        } else {
            out.skipped.push(Finding {
                path: shown,
                reason: Reason::NotAFileOrFolder,
            });
        }
    }
    Ok(())
}

fn read_subfolder(
    out: &mut Walk,
    stack: &mut Vec<Pending>,
    folder: &Pending,
    dir: &Dir,
    name: &OsStr,
    shown: &str,
    path: Option<ManifestPath>,
) -> Result<(), Error> {
    let handle = dir
        .open_dir(name)
        .map_err(|e| refused(shown, &format!("it cannot be opened ({})", e.kind())))?
        .into_file();
    let seen = prove(&handle, shown, Some(folder.volume))?;
    if let Some(path) = &path {
        out.items.push(Item {
            path: path.clone(),
            shown: shown.to_string(),
            what: What::Dir {
                mtime_ms: meta::mtime_ms(&seen.meta),
                mode: meta::mode(&seen.meta),
            },
        });
    }
    stack.push(Pending {
        root: folder.root.clone(),
        volume: folder.volume,
        rel: folder.rel.join(name),
        shown: shown.to_string(),
        path: path.map(|p| p.as_str().to_owned()),
    });
    Ok(())
}

fn add_file(
    out: &mut Walk,
    folder: &Pending,
    dir: &Dir,
    name: &OsStr,
    shown: String,
    path: ManifestPath,
) -> Result<(), Error> {
    let file = open_in(dir, name).map_err(|e| unreadable(&shown, &e))?;
    let seen = prove(&file, &shown, Some(folder.volume))?;
    if !seen.meta.is_file() {
        out.skipped.push(Finding {
            path: shown,
            reason: Reason::NotAFileOrFolder,
        });
        return Ok(());
    }
    if seen.links > 1 {
        return Err(refused(&shown, MORE_THAN_ONE_NAME));
    }
    out.items.push(Item {
        path,
        shown: shown.clone(),
        what: What::File {
            disk: PathBuf::from(&shown),
            below: Some(Below {
                root: folder.root.clone(),
                rel: folder.rel.join(name),
                id: seen.id,
            }),
        },
    });
    Ok(())
}
