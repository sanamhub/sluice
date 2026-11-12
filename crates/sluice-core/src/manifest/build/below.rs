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
use std::path::{Path, PathBuf};
use std::sync::Arc;

use unicode_normalization::UnicodeNormalization;

use super::argument::{nfc_name, unique_name};
use super::dirs::{Dir, Kind};
use super::walk::{Item, Walk, What};
use super::{Finding, Reason, meta};
use crate::Error;
use crate::engine::{ObjectId, facts, is_link};
use crate::manifest::path::{ManifestPath, component_rule};

/// A folder, or a file with the folder it is in, that a caller opened and proved.
#[derive(Debug)]
pub struct ConfinedArg {
    handle: File,
    name: String,
    file: bool,
}

impl ConfinedArg {
    /// A folder to send. `handle` is the open folder, and `name` is the name it arrives under.
    pub fn folder(handle: File, name: String) -> Self {
        Self {
            handle,
            name,
            file: false,
        }
    }

    /// A file to send. `parent` is the open folder it is in, and `name` is its name there.
    pub fn file(parent: File, name: String) -> Self {
        Self {
            handle: parent,
            name,
            file: true,
        }
    }
}

/// Where a confined walk found a file: the folder handle it is below, its path below that, and the
/// object it was.
#[derive(Clone, Debug)]
pub struct Below {
    root: Arc<Dir>,
    rel: PathBuf,
    id: ObjectId,
}

impl Below {
    /// Opens the file again, from the folder handle and without following a link anywhere on the
    /// way, and refuses it unless it is the object the walk saw.
    pub fn open(&self) -> io::Result<File> {
        let file = open_file_below(&self.root, &self.rel)?;
        if facts(&file)?.identity != self.id {
            return Err(io::Error::other("the file is not the object the walk saw"));
        }
        Ok(file)
    }
}

/// Opens the folder `rel` below `root`, one component at a time, never following a link.
pub(crate) fn open_dir_below(root: &Dir, rel: &Path) -> io::Result<Dir> {
    let mut here = root.try_clone()?;
    for part in rel.components() {
        here = here.open_dir(part.as_os_str())?;
    }
    Ok(here)
}

/// Opens the file `rel` below `root`, never following a link, the last component included.
pub(crate) fn open_file_below(root: &Dir, rel: &Path) -> io::Result<File> {
    let name = rel
        .file_name()
        .ok_or_else(|| io::Error::other("a file has a name"))?;
    let parent = open_dir_below(root, rel.parent().unwrap_or(Path::new("")))?;
    open_in(&parent, name)
}

fn open_in(parent: &Dir, name: &OsStr) -> io::Result<File> {
    parent.open_file(name)
}

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

/// Runs `hook` with the name of each folder just before the walk lists it, which is where a test
/// swaps a folder for a link, the one moment a real run cannot be made to hit on purpose.
#[cfg(test)]
pub(super) fn before_listing(hook: impl Fn(&str) + 'static) {
    BEFORE_LISTING.with(|slot| *slot.borrow_mut() = Box::new(hook));
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
            file_argument(&mut out, root, name)?;
        } else {
            let handle = root.try_clone().map_err(|e| unreadable(&shown, &e))?;
            folder_argument(&mut out, &mut stack, root, &handle, name)?;
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

fn file_argument(out: &mut Walk, parent: Arc<Dir>, name: String) -> Result<(), Error> {
    let file = open_in(&parent, OsStr::new(&name)).map_err(|e| unreadable(&name, &e))?;
    let seen = prove(&file, &name, None)?;
    if !seen.meta.is_file() {
        return Err(refused(&name, "it is not a regular file or a folder"));
    }
    if seen.links > 1 {
        return Err(refused(&name, MORE_THAN_ONE_NAME));
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
            disk: PathBuf::from(&name),
            below: Some(Below {
                root: parent,
                rel: PathBuf::from(&name),
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
) -> Result<(), Error> {
    let file = handle
        .try_clone()
        .map_err(|e| unreadable(&name, &e))?
        .into_file();
    let seen = prove(&file, &name, None)?;
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
    let names = dir.names().map_err(|e| unreadable(&folder.shown, &e))?;
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
