//! The folders a daemon may read from and write to, and the check that a request's path is inside
//! one. The server holds an open handle to each root from the moment it starts, and a path is
//! refused unless every component below the root is a name under it and none is a link.
//!
//! The check is made on the path and the transfer opens it afterwards, so a component swapped for
//! a link between the two is not caught. SPEC section 11 asks for an open relative to the
//! folder's handle, which the engine's path-taking calls cannot yet do.

use std::fs::{self, File};
use std::path::{Path, PathBuf};

use sluice_core::manifest::path::{ManifestPath, shown};

use crate::error::ApiError;
use crate::identity::{self, Identity};
use crate::proto::RootPath;

/// What a refusal says when the daemon has no root at all.
pub const NO_ROOTS: &str = "this daemon was started with no root folders, so it refuses every \
send and receive; start it with `sluice daemon --root DIR` to allow the folders it may use";

/// One root: the name it was given, the folder that name resolved to when the daemon started, and
/// the open handle that stands for that folder.
struct Root {
    name: String,
    path: PathBuf,
    // The proof of which folder the daemon opened: the name is compared with it on every request.
    held: File,
    identity: Identity,
    volume: u64,
}

/// The roots of one daemon.
#[derive(Default)]
pub struct Roots {
    roots: Vec<Root>,
}

/// What a path under a root is for.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Use {
    /// A file or folder that exists and is read: the path of a send.
    Read,
    /// A folder or file that may not exist yet and is created: an output folder, a receipt.
    Write,
}

impl Roots {
    /// No roots: every send and receive is refused.
    pub fn none() -> Self {
        Self::default()
    }

    /// Makes each folder absolute, resolves it once and opens it. A root that is not a folder, or
    /// that is given twice, is refused.
    pub fn open(dirs: &[PathBuf]) -> Result<Self, ApiError> {
        let mut roots: Vec<Root> = Vec::new();
        for dir in dirs {
            let name = dir
                .to_str()
                .ok_or_else(|| ApiError::Invalid("a root's name is not valid text".to_string()))?;
            if roots.iter().any(|root| root.name == name) {
                return Err(ApiError::Invalid(format!(
                    "the root {} is given twice",
                    shown(name)
                )));
            }
            roots.push(open_one(name, dir)?);
        }
        Ok(Self { roots })
    }

    /// Whether the daemon has no root.
    pub fn is_empty(&self) -> bool {
        self.roots.is_empty()
    }

    /// The path a request names, as an absolute path under a root, or the reason it is refused.
    pub(crate) fn resolve(&self, at: Option<&RootPath>, usage: Use) -> Result<PathBuf, ApiError> {
        if self.roots.is_empty() {
            return Err(ApiError::Refused(NO_ROOTS.to_string()));
        }
        let at = at.ok_or_else(|| ApiError::Invalid("the request names no path".to_string()))?;
        let root = self.find(&at.root)?;
        root.still_the_same()?;
        if at.path.is_empty() {
            return Ok(root.path.clone());
        }
        let parsed = ManifestPath::parse(&at.path).map_err(|e| ApiError::Refused(e.to_string()))?;
        let names: Vec<&str> = parsed.components().collect();
        let mut here = root.path.clone();
        for (index, name) in names.iter().enumerate() {
            here.push(name);
            match fs::symlink_metadata(&here) {
                Ok(meta) => check_component(&meta, root.volume, index + 1 == names.len())?,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    return missing(usage, here, &names[index + 1..]);
                }
                Err(e) => return Err(refuse_io("look at", &here, &e)),
            }
        }
        Ok(here)
    }

    fn find(&self, name: &str) -> Result<&Root, ApiError> {
        self.roots
            .iter()
            .find(|root| root.name == name)
            .ok_or_else(|| {
                let known: Vec<String> = self.roots.iter().map(|r| shown(&r.name)).collect();
                ApiError::Refused(format!(
                    "{} is not a root of this daemon; its roots are {}",
                    shown(name),
                    known.join(", ")
                ))
            })
    }
}

/// A path with a missing component is fine to write to, and the rest of it is joined as text:
/// nothing there can be a link. It is an error to read one.
fn missing(usage: Use, mut here: PathBuf, rest: &[&str]) -> Result<PathBuf, ApiError> {
    if usage == Use::Read {
        return Err(ApiError::Invalid(
            "a path to send does not exist inside its root".to_string(),
        ));
    }
    here.extend(rest);
    Ok(here)
}

/// Resolves, opens and records one root.
fn open_one(name: &str, dir: &Path) -> Result<Root, ApiError> {
    let path = plain(fs::canonicalize(dir).map_err(|e| refuse_io("resolve", dir, &e))?);
    let held = identity::open_folder(&path).map_err(|e| refuse_io("open", &path, &e))?;
    let meta = held
        .metadata()
        .map_err(|e| refuse_io("look at", &path, &e))?;
    if !meta.is_dir() {
        return Err(ApiError::Invalid(format!(
            "the root {} is not a folder",
            shown(name)
        )));
    }
    let facts = identity::facts(&held).map_err(|e| refuse_io("look at", &path, &e))?;
    Ok(Root {
        name: name.to_string(),
        path,
        held,
        identity: facts.identity,
        volume: identity::volume_of(&meta),
    })
}

impl Root {
    /// Refuses when the folder this root's name leads to is no longer the folder that was opened:
    /// replaced, renamed away, or gone.
    fn still_the_same(&self) -> Result<(), ApiError> {
        let by_name = identity::open_folder(&self.path)
            .and_then(|folder| identity::facts(&folder))
            .map_err(|_| self.replaced())?;
        let held = identity::facts(&self.held).map_err(|_| self.replaced())?;
        if by_name.identity == held.identity && held.identity == self.identity {
            Ok(())
        } else {
            Err(self.replaced())
        }
    }

    fn replaced(&self) -> ApiError {
        ApiError::Refused(format!(
            "the root {} is not the folder this daemon opened; start the daemon again",
            shown(&self.name)
        ))
    }
}

/// Checks one component that exists.
fn check_component(meta: &fs::Metadata, volume: u64, last: bool) -> Result<(), ApiError> {
    if is_link(meta) {
        return Err(ApiError::Refused(
            "a link is never followed inside a root, and this path has one".to_string(),
        ));
    }
    if identity::volume_of(meta) != volume {
        return Err(ApiError::Refused(
            "a path inside a root does not cross onto another volume".to_string(),
        ));
    }
    if !last && !meta.is_dir() {
        return Err(ApiError::Invalid(
            "a component of the path is a file, not a folder".to_string(),
        ));
    }
    Ok(())
}

/// Refuses a file with more than one name: its other name may lie outside every root, and no check
/// of this path could see that. The count is read from the handle the file is opened with.
pub(crate) fn refuse_hard_links(path: &Path) -> Result<(), ApiError> {
    let meta = fs::symlink_metadata(path).map_err(|e| refuse_io("look at", path, &e))?;
    if !meta.is_file() {
        return Ok(());
    }
    let file = identity::open_file(path).map_err(|e| refuse_io("open", path, &e))?;
    let facts = identity::facts(&file).map_err(|e| refuse_io("look at", path, &e))?;
    if facts.links > 1 {
        return Err(ApiError::Refused(
            "a file with more than one name may also be reachable from outside every root, so \
             this daemon does not send it"
                .to_string(),
        ));
    }
    Ok(())
}

/// A symbolic link, and on Windows any reparse point: a junction, a mount point or a link, which
/// a path under a root never goes through.
fn is_link(meta: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const REPARSE_POINT: u32 = 0x400;
        meta.file_attributes() & REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        meta.file_type().is_symlink()
    }
}

/// The path without the verbatim prefix `canonicalize` gives on Windows, which the engine's own
/// path handling does not expect.
fn plain(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        let text = path.to_string_lossy();
        if let Some(rest) = text.strip_prefix(r"\\?\")
            && rest.as_bytes().get(1) == Some(&b':')
        {
            return PathBuf::from(rest.to_string());
        }
    }
    path
}

fn refuse_io(what: &str, path: &Path, error: &std::io::Error) -> ApiError {
    ApiError::Refused(format!(
        "cannot {what} {}: {}",
        shown(&path.display().to_string()),
        shown(&error.to_string())
    ))
}

#[cfg(test)]
mod tests;
