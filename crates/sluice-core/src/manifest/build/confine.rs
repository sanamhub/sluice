//! The confined mode of the walk, which a daemon always uses: every entry of a folder it sends is
//! judged from its own handle, so nothing below the folder can lead out of the root it was opened
//! under. A link, a file with more than one name, and an entry on another volume refuse the send.

use std::fs::FileType;
use std::path::Path;

use crate::Error;
use crate::engine::{ObjectId, facts, is_link, open_file, open_folder};

/// The volume of the folder a confined walk started in.
#[derive(Clone, Copy, Debug)]
pub(super) struct Confine {
    volume: u64,
}

fn refused(shown: &str, why: &str) -> Error {
    Error::Policy(format!(
        "{} is refused: {why}",
        crate::manifest::path::shown(shown)
    ))
}

impl Confine {
    /// The confinement of a walk that starts in the folder at `start`.
    pub(super) fn at(start: &Path) -> Result<Self, Error> {
        let id = ObjectId::of_path(start)
            .map_err(|e| Error::Storage(format!("cannot read the path to send: {}", e.kind())))?;
        Ok(Self {
            volume: id.volume(),
        })
    }

    /// Refuses an entry that is a link, is on another volume, or is a file with more than one name.
    /// What it reads comes from a handle opened without following a link, never from the path.
    pub(super) fn check(&self, path: &Path, kind: FileType, shown: &str) -> Result<(), Error> {
        if kind.is_symlink() {
            return Err(refused(shown, "it is a link, which a daemon never sends"));
        }
        let handle = if kind.is_dir() {
            open_folder(path)
        } else {
            open_file(path)
        }
        .map_err(|e| refused(shown, &format!("it cannot be opened ({})", e.kind())))?;
        if is_link(&handle)
            .map_err(|e| refused(shown, &format!("it cannot be read ({})", e.kind())))?
        {
            return Err(refused(shown, "it is a link, which a daemon never sends"));
        }
        let found = facts(&handle)
            .map_err(|e| refused(shown, &format!("it cannot be read ({})", e.kind())))?;
        if found.identity.volume() != self.volume {
            return Err(refused(
                shown,
                "it is on another volume than the folder sent",
            ));
        }
        if kind.is_file() && found.links > 1 {
            return Err(refused(
                shown,
                "it has more than one name, and another may lie outside the root",
            ));
        }
        Ok(())
    }
}
