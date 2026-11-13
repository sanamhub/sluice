//! What a caller hands the confined walk, and what the walk hands back: the handles of the folders
//! and files it proved, and where a file was found, with the opens that find it again below a folder
//! handle without following a link.

use std::ffi::OsStr;
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::dirs::Dir;
use crate::engine::{ObjectId, facts};

/// A folder, or a file with the folder it is in, that a caller opened and proved.
#[derive(Debug)]
pub struct ConfinedArg {
    pub(super) handle: File,
    pub(super) name: String,
    pub(super) file: bool,
    /// The object the caller proved this to be, which the walk's own look at it must agree with.
    pub(super) expect: Option<ObjectId>,
}

impl ConfinedArg {
    /// A folder to send. `handle` is the open folder, and `name` is the name it arrives under.
    pub fn folder(handle: File, name: String) -> Self {
        Self {
            handle,
            name,
            file: false,
            expect: None,
        }
    }

    /// A file to send. `parent` is the open folder it is in, and `name` is its name there.
    pub fn file(parent: File, name: String) -> Self {
        Self {
            handle: parent,
            name,
            file: true,
            expect: None,
        }
    }

    /// Says which object the caller proved: the walk refuses the argument if the handle, or the
    /// file found by name below the folder, is another object.
    pub fn expecting(mut self, identity: ObjectId) -> Self {
        self.expect = Some(identity);
        self
    }
}

/// Where a confined walk found a file: the folder handle it is below, its path below that, and the
/// object it was.
#[derive(Clone, Debug)]
pub struct Below {
    pub(super) root: Arc<Dir>,
    pub(super) rel: PathBuf,
    pub(super) id: ObjectId,
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
pub(super) fn open_dir_below(root: &Dir, rel: &Path) -> io::Result<Dir> {
    let mut here = root.try_clone()?;
    for part in rel.components() {
        here = here.open_dir(part.as_os_str())?;
    }
    Ok(here)
}

/// Opens the file `rel` below `root`, never following a link, the last component included.
pub(super) fn open_file_below(root: &Dir, rel: &Path) -> io::Result<File> {
    let name = rel
        .file_name()
        .ok_or_else(|| io::Error::other("a file has a name"))?;
    let parent = open_dir_below(root, rel.parent().unwrap_or(Path::new("")))?;
    open_in(&parent, name)
}

pub(super) fn open_in(parent: &Dir, name: &OsStr) -> io::Result<File> {
    parent.open_file(name)
}
