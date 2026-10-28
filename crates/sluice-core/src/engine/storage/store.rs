//! The folder one receive keeps its partial store in: where it sits, what its marker says, who may
//! read it, and how the folder that was checked is known to be the folder that was opened.

use std::{
    collections::BTreeSet,
    fs, io,
    path::{Path, PathBuf},
};

use super::{registry, remove_quietly, storage};
use crate::Error;

/// The name of the file inside a partial store that says which ticket made it, which top-level
/// entries it made room for, and whether it ran with `--force`. Without it a store left by a failed
/// run cannot be told from one another ticket made under a name that happens to match, and a resume
/// would trust bytes nobody checked.
pub const MARKER: &str = "ticket";

/// The hidden store beside the destination. A per-transfer store keeps partial data on the same
/// filesystem as the file it will become, so a finished blob can be moved into place. The name is
/// the whole root, so two transfers cannot share one by a prefix of their root.
pub fn partial_dir(out: &Path, root: &str) -> PathBuf {
    out.join(format!(".sluice-partial-{root}"))
}

/// What a partial store's marker says: the transfer that made the folder, the top-level names that
/// transfer created in the output, and whether it was allowed to write over what was there. A resume
/// may write into those names and no others, and only if it is as deliberate as that run was.
#[derive(Debug)]
pub struct Owner {
    pub root: String,
    pub names: BTreeSet<String>,
    pub force: bool,
    /// The id this home's record of the store carries, so a folder that was replaced under the
    /// same name is not taken for the one the record names.
    pub id: Option<String>,
}

/// Reads the marker of `dir` without changing anything, after checking that the folder is a real
/// folder of this user. A folder with no marker, or one this user cannot read, is not a resume;
/// the caller deals with it once every check has passed.
pub fn recorded(dir: &Path) -> Result<Option<Owner>, Error> {
    match fs::symlink_metadata(dir) {
        Ok(_) => crate::home::check_folder(dir)?,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(storage("read", dir, e)),
    }
    let marker = dir.join(MARKER);
    let text = match fs::read_to_string(&marker) {
        Ok(text) => text,
        Err(e)
            if matches!(
                e.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::PermissionDenied
            ) =>
        {
            return Ok(None);
        }
        Err(e) => return Err(storage("read", &marker, e)),
    };
    Ok(parse_marker(&text))
}

/// The first line is the root; every `name ` line after it is a top-level entry the transfer
/// created, an `id ` line is the record this store is named by, and a `force` line records that the
/// run that made the store was allowed to replace what was already there.
fn parse_marker(text: &str) -> Option<Owner> {
    let mut lines = text.lines();
    let root = lines.next()?.trim();
    if root.is_empty() {
        return None;
    }
    let mut names = BTreeSet::new();
    let mut force = false;
    let mut id = None;
    for line in lines {
        if let Some(name) = line.strip_prefix("name ") {
            names.insert(name.to_string());
        }
        if let Some(record) = line.strip_prefix("id ") {
            id = Some(record.trim().to_string());
        }
        force |= line.trim() == "force";
    }
    Some(Owner {
        root: root.to_string(),
        names,
        force,
        id,
    })
}

/// Makes `dir` an empty store of this receive, removing a folder that is not one. Called only after
/// every check has passed, so a refusal leaves no store for a later run to resume.
///
/// The folder is made with one `create_dir`, which fails on a name that exists, so a folder another
/// process put there between the checks is never claimed, and is refused rather than written into.
/// A folder this call made is then made private: another user who can write in the output could
/// otherwise read the marker, and a store whose marker can be read can be copied and swapped in.
pub fn claim(dir: &Path) -> Result<(), Error> {
    remove_quietly(dir);
    match fs::create_dir(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
            return Err(registry::clash(dir));
        }
        Err(e) => return Err(storage("create", dir, e)),
    }
    crate::home::restrict_folder(dir)
}

/// Writes the marker of a store this receive just made, naming the top-level entries it creates,
/// whether it may replace what is there, and the id this home's record of the store carries. A
/// later run reads it to tell this receive's folders from the user's, to refuse to continue a run
/// that was forced when it is not forced itself, and to match the store to its record.
pub fn remember(
    dir: &Path,
    root: &str,
    names: &BTreeSet<String>,
    force: bool,
    id: &str,
) -> Result<(), Error> {
    let marker = dir.join(MARKER);
    let mut text = format!("{root}\nid {id}\n");
    if force {
        text.push_str("force\n");
    }
    for name in names {
        text.push_str("name ");
        text.push_str(name);
        text.push('\n');
    }
    fs::write(&marker, text).map_err(|e| storage("write", &marker, e))
}

/// What names one folder on this machine, independent of its path: a folder renamed away and
/// another put under the same name is a different folder, and its numbers differ.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FolderId {
    first: u64,
    second: u64,
}

/// The identity of `dir`, or `None` on a platform with no call for it. Read once before a folder is
/// checked and once after its store is opened, so the store that was opened is the store that was
/// checked.
pub fn identify(dir: &Path) -> Result<Option<FolderId>, Error> {
    imp::identity(dir).map_err(|e| storage("read", dir, e))
}

#[cfg(unix)]
mod imp {
    use std::{fs, io, os::unix::fs::MetadataExt, path::Path};

    use super::FolderId;

    pub(super) fn identity(dir: &Path) -> io::Result<Option<FolderId>> {
        let meta = fs::symlink_metadata(dir)?;
        Ok(Some(FolderId {
            first: meta.dev(),
            second: meta.ino(),
        }))
    }
}

#[cfg(windows)]
mod imp {
    use std::{io, path::Path};

    use super::FolderId;

    pub(super) fn identity(dir: &Path) -> io::Result<Option<FolderId>> {
        let (volume, index) = crate::home::private_windows::folder_id(dir)?;
        Ok(Some(FolderId {
            first: volume,
            second: index,
        }))
    }
}

#[cfg(not(any(unix, windows)))]
mod imp {
    use std::{io, path::Path};

    use super::FolderId;

    pub(super) fn identity(_dir: &Path) -> io::Result<Option<FolderId>> {
        Ok(None)
    }
}
