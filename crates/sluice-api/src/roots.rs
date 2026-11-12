//! The folders a daemon may read from and write to, and the open of a request's path inside one.
//! The server holds an open handle to each root from the moment it starts. A path is a root's name
//! and the names below it, and every one of them is opened from the handle of the folder before it
//! (`walk`), never by joining text and opening the result. What the open returns is handed on with
//! the identity of the object it opened, which the engine checks where it opens the same path.

use std::fs::File;
use std::path::{Path, PathBuf};

use sluice_core::engine::{ObjectId, facts, open_folder, volume_of};
use sluice_core::manifest::path::{ManifestPath, shown};

use crate::error::ApiError;
use crate::proto::RootPath;

mod walk;

pub(crate) use walk::{Opened, Use};

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
    identity: ObjectId,
    volume: u64,
}

/// The roots of one daemon.
#[derive(Default)]
pub struct Roots {
    roots: Vec<Root>,
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

    /// Refuses a root that is the home, holds it, or lies inside it. A root is a folder clients may
    /// write into and read from, and the home holds the identity key and the stores, so a root that
    /// reaches any of it would hand them to every program that runs as this user.
    pub fn refuse_home(&self, home: &Path) -> Result<(), ApiError> {
        let home = plain(std::fs::canonicalize(home).map_err(|e| refuse_io("resolve", home, &e))?);
        for root in &self.roots {
            if overlaps(&root.path, &home) {
                return Err(ApiError::Refused(format!(
                    "the root {} is the daemon's home, holds it, or lies inside it, and a root \
                     must be a folder of its own: the home keeps the identity key and the stores",
                    shown(&root.name)
                )));
            }
        }
        Ok(())
    }

    /// Whether the daemon has no root.
    pub fn is_empty(&self) -> bool {
        self.roots.is_empty()
    }

    /// Opens the path a request names, one component at a time from its root's handle, or says why
    /// it is refused.
    pub(crate) fn resolve(&self, at: Option<&RootPath>, usage: Use) -> Result<Opened, ApiError> {
        if self.roots.is_empty() {
            return Err(ApiError::Refused(NO_ROOTS.to_string()));
        }
        let at = at.ok_or_else(|| ApiError::Invalid("the request names no path".to_string()))?;
        let root = self.find(&at.root)?;
        root.still_the_same()?;
        if at.path.is_empty() {
            return root.itself(usage);
        }
        let parsed = ManifestPath::parse(&at.path).map_err(|e| ApiError::Refused(e.to_string()))?;
        let names: Vec<&str> = parsed.components().collect();
        let start = walk::Start {
            held: &root.held,
            path: &root.path,
            volume: root.volume,
        };
        walk::walk(&start, &names, usage)
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

/// Resolves, opens and records one root.
fn open_one(name: &str, dir: &Path) -> Result<Root, ApiError> {
    let path = plain(std::fs::canonicalize(dir).map_err(|e| refuse_io("resolve", dir, &e))?);
    let held = open_folder(&path).map_err(|e| refuse_io("open", &path, &e))?;
    let meta = held
        .metadata()
        .map_err(|e| refuse_io("look at", &path, &e))?;
    if !meta.is_dir() {
        return Err(ApiError::Invalid(format!(
            "the root {} is not a folder",
            shown(name)
        )));
    }
    let found = facts(&held).map_err(|e| refuse_io("look at", &path, &e))?;
    Ok(Root {
        name: name.to_string(),
        path,
        held,
        identity: found.identity,
        volume: volume_of(&meta),
    })
}

impl Root {
    /// Refuses when the folder this root's name leads to is no longer the folder that was opened:
    /// replaced, renamed away, or gone.
    fn still_the_same(&self) -> Result<(), ApiError> {
        let by_name = open_folder(&self.path)
            .and_then(|folder| facts(&folder))
            .map_err(|_| self.replaced())?;
        let held = facts(&self.held).map_err(|_| self.replaced())?;
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

    /// The root folder itself, which a request names with an empty path.
    fn itself(&self, usage: Use) -> Result<Opened, ApiError> {
        if usage == Use::FileName {
            return Err(ApiError::Invalid(
                "a file is named below a root, not as the root itself".to_string(),
            ));
        }
        let found = facts(&self.held).map_err(|e| refuse_io("look at", &self.path, &e))?;
        let held = self
            .held
            .try_clone()
            .map_err(|e| refuse_io("look at", &self.path, &e))?;
        Ok(Opened {
            path: self.path.clone(),
            pin: found.identity,
            links: found.links,
            is_file: false,
            _held: vec![held],
        })
    }
}

/// Whether one path is the other or is inside it, in either direction. Windows file names are
/// compared without regard to case.
fn overlaps(a: &Path, b: &Path) -> bool {
    let fold = |p: &Path| {
        let text = p.to_string_lossy();
        PathBuf::from(if cfg!(windows) {
            text.to_lowercase()
        } else {
            text.into_owned()
        })
    };
    let (a, b) = (fold(a), fold(b));
    a.starts_with(&b) || b.starts_with(&a)
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
