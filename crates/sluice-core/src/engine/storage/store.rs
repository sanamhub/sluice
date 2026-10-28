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

/// What a folder that may hold a store says: its marker, when it has one, and the identity of the
/// folder read while that marker was read. The identity is taken inside the check, from the handle
/// that checked the folder's kind and owner, so a caller can compare it with the folder the store
/// was actually opened in.
#[derive(Debug)]
pub struct Found {
    /// The marker.
    pub owner: Owner,
    pub id: Option<FolderId>,
}

/// Reads the marker of `dir` without changing anything, after checking that the folder is a real
/// folder of this user. A folder with no marker, or one this user cannot read, is not a resume;
/// the caller deals with it once every check has passed.
pub fn recorded(dir: &Path) -> Result<Option<Found>, Error> {
    let id = match fs::symlink_metadata(dir) {
        Ok(_) => crate::home::check_folder_id(dir)?.map(FolderId::of),
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(storage("read", dir, e)),
    };
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
    Ok(parse_marker(&text).map(|owner| Found { owner, id }))
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
/// The folder is made with the private builder, which fails on a name that exists and makes the
/// folder mode 0700 as it is created: another user who can write in the output could otherwise read
/// the marker, and a store whose marker can be read can be copied and swapped in. The access list,
/// where the platform has one, is set next through a handle, so the folder whose kind and owner are
/// checked is the folder that gets the list. A folder another process put there between the checks
/// is never claimed, and is refused rather than written into.
pub fn claim(dir: &Path) -> Result<(), Error> {
    remove_quietly(dir);
    match crate::home::create_private_dir(dir) {
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
    write_marker(&marker, &text).map_err(|e| storage("write", &marker, e))
}

/// Writes a marker at a name that must not exist yet, and never through a link: the store folder is
/// this user's, but a `ticket` link planted in it would otherwise be written through to wherever it
/// points. `O_EXCL` and `create_new` refuse a name that is there; `NOFOLLOW` and
/// `OPEN_REPARSE_POINT` refuse a link at it.
fn write_marker(marker: &Path, text: &str) -> io::Result<()> {
    use std::io::Write;

    let mut file = open_new_marker(marker)?;
    file.write_all(text.as_bytes())
}

#[cfg(unix)]
fn open_new_marker(marker: &Path) -> io::Result<fs::File> {
    use rustix::fs::{Mode, OFlags};

    // Mode 0600 at creation: the marker holds a ticket root, which names content this home is
    // receiving, and nothing outside this user needs to read it.
    let fd = rustix::fs::open(
        marker,
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::from_raw_mode(0o600),
    )?;
    Ok(fs::File::from(fd))
}

#[cfg(windows)]
fn open_new_marker(marker: &Path) -> io::Result<fs::File> {
    use std::os::windows::fs::OpenOptionsExt;

    const OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .custom_flags(OPEN_REPARSE_POINT)
        .open(marker)
}

#[cfg(not(any(unix, windows)))]
fn open_new_marker(marker: &Path) -> io::Result<fs::File> {
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(marker)
}

/// What names one folder on this machine, independent of its path: a folder renamed away and
/// another put under the same name is a different folder, and its numbers differ.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FolderId {
    first: u64,
    second: u64,
}

impl FolderId {
    pub(crate) fn of((first, second): (u64, u64)) -> Self {
        Self { first, second }
    }
}

/// The identity of `dir`, or `None` on a platform with no call for it. Used for a file as well as a
/// folder: a followed file that was replaced is a different file, whatever its path says.
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
