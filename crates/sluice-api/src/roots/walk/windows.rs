//! The walk on Windows. Each component is opened as itself, never through a reparse point, with a
//! share mode that lets nobody rename or delete it while the walk's handles are held, and is then
//! proved from its own handle to be a child of its parent's handle: its final path must be the
//! parent's final path and one more name. A component that is a reparse point of any kind is
//! refused, which is stricter than the name-surrogate rule of `SPEC.md` section 11 asks.
#![allow(unsafe_code)]

use std::fs::{File, OpenOptions};
use std::io;
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};

use sluice_core::engine::facts;
use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ, FILE_SHARE_WRITE,
    GetFinalPathNameByHandleW,
};

use super::{NOT_PLAIN, Opened, Start, Use};
use crate::error::ApiError;

const REPARSE_POINT: u32 = 0x400;

fn io_error(error: &io::Error) -> ApiError {
    ApiError::Refused(format!("cannot open the path: {error}"))
}

fn refused() -> ApiError {
    ApiError::Refused(NOT_PLAIN.to_string())
}

/// Opens `path` as itself. No share mode allows a delete or a rename, so the component stays where
/// it is for as long as the handle is held.
fn open_as_itself(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
}

/// The path the system gives an open handle, without the `\\?\` prefix of a drive path.
fn final_path(file: &File) -> io::Result<PathBuf> {
    let mut buffer = vec![0u16; 512];
    loop {
        // SAFETY: the buffer holds `buffer.len()` units, which the call is told, and the handle is
        // open for as long as `file` is borrowed.
        let written = unsafe {
            GetFinalPathNameByHandleW(
                file.as_raw_handle() as HANDLE,
                buffer.as_mut_ptr(),
                buffer.len() as u32,
                0,
            )
        } as usize;
        if written == 0 {
            return Err(io::Error::last_os_error());
        }
        if written < buffer.len() {
            let text = String::from_utf16(&buffer[..written]).map_err(io::Error::other)?;
            return Ok(PathBuf::from(match text.strip_prefix(r"\\?\") {
                Some(rest) if rest.as_bytes().get(1) == Some(&b':') => rest.to_string(),
                _ => text,
            }));
        }
        buffer.resize(written + 1, 0);
    }
}

fn same_path(a: &Path, b: &Path) -> bool {
    let fold = |p: &Path| p.to_string_lossy().to_lowercase();
    fold(a).trim_end_matches('\\') == fold(b).trim_end_matches('\\')
}

/// Opens `name` under the folder whose proved path is `parent`, and proves the result.
fn open_child(
    parent: &Path,
    name: &str,
    want_dir: bool,
    usage: Use,
) -> Result<(File, PathBuf), ApiError> {
    let wanted = parent.join(name);
    let file = match open_as_itself(&wanted) {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound && usage != Use::Read => {
            // Made with no replacing: a folder that appears in between is opened and proved.
            match std::fs::create_dir(&wanted) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(io_error(&e)),
            }
            open_as_itself(&wanted).map_err(|e| io_error(&e))?
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Err(ApiError::Invalid(
                "a path to send does not exist inside its root".to_string(),
            ));
        }
        Err(e) => return Err(io_error(&e)),
    };
    let meta = file.metadata().map_err(|e| io_error(&e))?;
    if meta.file_attributes() & REPARSE_POINT != 0 || (want_dir && !meta.is_dir()) {
        return Err(refused());
    }
    let proved = final_path(&file).map_err(|e| io_error(&e))?;
    // The component is a child of the folder it was opened under, by its own handle: a folder that
    // was swapped for a junction or another folder in between shows as another parent.
    if proved
        .parent()
        .is_none_or(|above| !same_path(above, parent))
    {
        return Err(refused());
    }
    Ok((file, proved))
}

/// Opens every component of `names` below the root.
pub(in crate::roots) fn walk(
    start: &Start<'_>,
    names: &[&str],
    usage: Use,
) -> Result<Opened, ApiError> {
    let mut path = final_path(start.held).map_err(|e| io_error(&e))?;
    if !same_path(&path, start.path) {
        return Err(refused());
    }
    let mut held: Vec<File> = Vec::new();
    let mut current = start.held.try_clone().map_err(|e| io_error(&e))?;
    for (index, name) in names.iter().enumerate() {
        let last = index + 1 == names.len();
        if last && usage == Use::FileName {
            return name_only(current, path, name, held);
        }
        let want_dir = !(last && usage == Use::Read);
        let (next, proved) = open_child(&path, name, want_dir, usage)?;
        held.push(std::mem::replace(&mut current, next));
        path = proved;
    }
    let meta = current.metadata().map_err(|e| io_error(&e))?;
    let found = facts(&current).map_err(|e| io_error(&e))?;
    held.push(current);
    Ok(Opened {
        path,
        pin: found.identity,
        links: found.links,
        is_file: meta.is_file(),
        _held: held,
    })
}

/// The last name of a path whose file the engine creates: it is not opened, and is refused if it is
/// already there as a reparse point.
fn name_only(
    parent: File,
    path: PathBuf,
    name: &str,
    mut held: Vec<File>,
) -> Result<Opened, ApiError> {
    let wanted = path.join(name);
    match std::fs::symlink_metadata(&wanted) {
        Ok(meta) if meta.file_attributes() & REPARSE_POINT != 0 => return Err(refused()),
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(io_error(&e)),
    }
    let found = facts(&parent).map_err(|e| io_error(&e))?;
    held.push(parent);
    Ok(Opened {
        path: wanted,
        pin: found.identity,
        links: 1,
        is_file: false,
        _held: held,
    })
}
