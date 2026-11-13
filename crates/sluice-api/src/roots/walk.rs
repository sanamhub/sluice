//! The open of a path below a root, one component at a time, from the handle of the folder before
//! it. Nothing is joined as text and opened afterwards: each component is opened relative to its
//! parent's handle, or on Windows opened and then proved, from its own handle, to be a child of the
//! parent's handle. Every handle the walk opened is kept in the result, so the chain it proved stays
//! in place for as long as the caller holds it.

use std::fs::File;
use std::path::{Path, PathBuf};

use sluice_core::engine::ObjectId;
use sluice_core::manifest::path::shown;

use crate::error::ApiError;

#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub(super) use unix::walk;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub(super) use windows::walk;

/// What a path under a root is for.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Use {
    /// A file or folder that exists and is read: the path of a send.
    Read,
    /// A folder that is written into. A component that is missing is made, with no replacing.
    Dir,
    /// A file the engine creates, such as a receipt: the folders above its name are made as for
    /// `Dir`, and the name itself is left for the engine, after it is checked not to be a link.
    FileName,
}

/// The root a walk starts from.
pub(super) struct Start<'a> {
    pub held: &'a File,
    pub path: &'a Path,
    #[cfg_attr(windows, allow(dead_code))]
    pub volume: u64,
}

/// A path below a root, opened by handle.
#[derive(Debug)]
pub(crate) struct Opened {
    /// The path of the object as the system reports it, for the engine to be given.
    pub path: PathBuf,
    /// The object the engine must find at `path` (`Read` and `Dir`), or at its parent (`FileName`).
    pub pin: ObjectId,
    /// How many names the object has. One for anything but a file with another name.
    pub links: u64,
    /// Whether the object is a regular file.
    pub is_file: bool,
    /// The open folder the object is, or for a file the open folder it is in: the handle a confined
    /// send lists and opens from, which is never turned back into a path.
    pub(super) handle: File,
    /// The name the object arrives under, which is its name below that handle for a file.
    pub(super) name: String,
    // Every handle the walk opened, which keeps the chain it proved in place where the platform lets
    // an open handle do that.
    pub(super) _held: Vec<File>,
}

impl Opened {
    /// The folder or file as the engine's confined walk takes it, from the handles this walk
    /// proved.
    pub(crate) fn confined_arg(&self) -> Result<sluice_core::manifest::ConfinedArg, ApiError> {
        let handle = self.handle.try_clone().map_err(|e| {
            ApiError::Refused(format!("cannot use the path: {}", shown(&e.to_string())))
        })?;
        let name = self.name.clone();
        let arg = if self.is_file {
            sluice_core::manifest::ConfinedArg::file(handle, name)
        } else {
            sluice_core::manifest::ConfinedArg::folder(handle, name)
        };
        Ok(arg.expecting(self.pin))
    }

    /// Refuses a file with more than one name: its other name may lie outside every root, and no
    /// check of this path could see that. The count is read from the handle the file was opened
    /// with.
    pub(crate) fn refuse_hard_links(&self) -> Result<(), ApiError> {
        if self.is_file && self.links > 1 {
            return Err(ApiError::Refused(
                "a file with more than one name may also be reachable from outside every root, so \
                 this daemon does not send it"
                    .to_string(),
            ));
        }
        Ok(())
    }
}

/// What a refused component says.
pub(super) const NOT_PLAIN: &str = "a link is never followed inside a root, and this path has a \
link or a component that is not a plain folder on the root's volume";
