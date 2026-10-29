//! One file's install: a private temporary name, its metadata set through the open handle, one
//! rename into place, and the content comparison that lets a resumed receive skip a file it wrote.

mod meta;
mod open;

use meta::{set_mode, set_time};
pub(super) use meta::{set_mode_path, set_time_path};
use open::{open_for_metadata, open_no_follow};

use std::{fs, io::Read, path::Path};

use iroh_blobs::{Hash, api::blobs::BlobReader};
use tokio::{
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
    sync::mpsc::UnboundedSender,
};

use super::super::{Event, storage};
use crate::{
    Error,
    manifest::{Entry, Loc},
};

/// The metadata of one entry, kept together so the write helpers have fewer arguments.
#[derive(Clone, Copy)]
pub(super) struct Meta {
    pub mtime_ms: u64,
    pub mode: u32,
}

/// An empty file, made with `create_new` so a name that appeared under the temporary name is an
/// error and not something this write opens.
pub(super) fn create_empty(path: &Path) -> Result<(), Error> {
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| storage::storage("create", path, e))?;
    Ok(())
}

/// Applies the metadata of a file this write just made and syncs it before the rename.
pub(super) fn finish(
    path: &Path,
    meta: Meta,
    no_times: bool,
    events: &UnboundedSender<Event>,
) -> Result<(), Error> {
    let file = fs::File::options()
        .write(true)
        .open(path)
        .map_err(|e| storage::storage("open", path, e))?;
    apply(&file, path, meta, no_times, events)?;
    file.sync_all()
        .map_err(|e| storage::storage("sync", path, e))
}

/// Sets the time and mode on a file that is already in place, which is what a resumed receive does
/// for a file whose content it recognized.
pub(super) fn touch(
    path: &Path,
    meta: Meta,
    no_times: bool,
    events: &UnboundedSender<Event>,
) -> Result<(), Error> {
    let file = open_for_metadata(path).map_err(|e| storage::storage("open", path, e))?;
    apply(&file, path, meta, no_times, events)
}

/// Mode and time on the open file, before it has its final name.
pub(super) fn apply(
    file: &fs::File,
    path: &Path,
    meta: Meta,
    no_times: bool,
    events: &UnboundedSender<Event>,
) -> Result<(), Error> {
    if !no_times {
        set_time(file, path, meta.mtime_ms, events)?;
    }
    set_mode(file, path, meta.mode)
}

/// Copies one `Packed` entry out of its pack into `incoming`, and returns the open file so its
/// metadata can be set before the rename.
pub(super) async fn copy_from_pack(
    reader: &mut BlobReader,
    entry: &Entry,
    incoming: &Path,
    write_limit: Option<u64>,
) -> Result<fs::File, Error> {
    let Loc::Packed { offset, len, .. } = entry.loc else {
        return Err(Error::Bug("a packed entry has no pack range".to_string()));
    };
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(incoming)
        .await
        .map_err(|e| storage::storage("create", incoming, e))?;
    reader
        .seek(std::io::SeekFrom::Start(offset))
        .await
        .map_err(|e| storage::storage("read a pack", incoming, e))?;
    let copied = copy_limited(reader, &mut file, len, incoming, write_limit).await?;
    file.flush()
        .await
        .map_err(|e| storage::storage("write", incoming, e))?;
    let file = file.into_std().await;
    if copied != len {
        return Err(Error::Verify(format!(
            "a packed file has {copied} bytes and the manifest says {len}"
        )));
    }
    Ok(file)
}

/// Copies `len` bytes from `reader` to `file`, and fails the way a full disk does once
/// `write_limit` bytes are written. Without a limit this is one `tokio::io::copy`.
async fn copy_limited(
    reader: &mut BlobReader,
    file: &mut tokio::fs::File,
    len: u64,
    incoming: &Path,
    limit: Option<u64>,
) -> Result<u64, Error> {
    let Some(limit) = limit else {
        return tokio::io::copy(&mut reader.take(len), file)
            .await
            .map_err(|e| storage::storage("write", incoming, e));
    };
    let mut buffer = vec![0u8; 64 * 1024];
    let mut copied = 0u64;
    while copied < len {
        let take = (len - copied).min(buffer.len() as u64);
        if copied + take > limit {
            return Err(Error::Storage(format!(
                "cannot write {}: no space left on device",
                incoming.display()
            )));
        }
        let take = take as usize;
        reader
            .read_exact(&mut buffer[..take])
            .await
            .map_err(|e| storage::storage("read a pack", incoming, e))?;
        file.write_all(&buffer[..take])
            .await
            .map_err(|e| storage::storage("write", incoming, e))?;
        copied += take as u64;
    }
    Ok(copied)
}

/// Whether `target` already holds the bytes of this pack range, which is how a receive that was
/// killed after writing a file recognises it instead of refusing it as a clash. The file is
/// checked for its kind and size before anything is read, a link is never read, and the bytes are
/// compared in bounded pieces so a target of another size cannot make this read all of it.
pub(super) async fn same_packed(
    reader: &mut BlobReader,
    entry: &Entry,
    target: &Path,
) -> Result<bool, Error> {
    let Loc::Packed { offset, len, .. } = entry.loc else {
        return Ok(false);
    };
    let meta = fs::symlink_metadata(target).map_err(|e| storage::storage("read", target, e))?;
    if !meta.is_file() || meta.len() != len {
        return Ok(false);
    }
    let mut file = open_no_follow(target).map_err(|e| storage::storage("read", target, e))?;
    reader
        .seek(std::io::SeekFrom::Start(offset))
        .await
        .map_err(|e| storage::storage("read a pack", target, e))?;
    // On the heap: a 64 KiB array in the future of this call would be copied onto the stack at
    // every poll, and the writer nests enough calls that a debug build runs out of stack.
    let mut expected = vec![0u8; 64 * 1024];
    let mut found = vec![0u8; 64 * 1024];
    let mut left = len;
    while left > 0 {
        let take = left.min(expected.len() as u64) as usize;
        file.read_exact(&mut found[..take])
            .map_err(|e| storage::storage("read", target, e))?;
        reader
            .read_exact(&mut expected[..take])
            .await
            .map_err(|e| storage::storage("read a pack", target, e))?;
        if found[..take] != expected[..take] {
            return Ok(false);
        }
        left -= take as u64;
    }
    Ok(true)
}

/// Whether a target already holds exactly this blob's content. A link is already a clash to the
/// caller, and it is never opened here either.
pub(super) fn same_blob(target: &Path, hash: Hash, size: u64) -> Result<bool, Error> {
    let meta = fs::symlink_metadata(target).map_err(|e| storage::storage("read", target, e))?;
    if !meta.is_file() || meta.len() != size {
        return Ok(false);
    }
    if size == 0 {
        return Ok(true);
    }
    let mut file = open_no_follow(target).map_err(|e| storage::storage("read", target, e))?;
    let mut hasher = blake3::Hasher::new();
    std::io::copy(&mut file, &mut hasher).map_err(|e| storage::storage("read", target, e))?;
    Ok(*hasher.finalize().as_bytes() == *hash.as_bytes())
}

#[cfg(all(test, unix))]
mod tests {
    use super::same_blob;
    use iroh_blobs::Hash;
    use std::fs;

    /// A link at a target is a clash, not content: resolving it and reading through it would let a
    /// link someone placed in the output stand in for a file the sender wrote.
    #[test]
    fn a_link_is_not_read_as_the_file_it_points_at() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real.bin");
        let body = b"the content";
        fs::write(&real, body).unwrap();
        let hash = Hash::from(*blake3::hash(body).as_bytes());
        std::os::unix::fs::symlink(&real, dir.path().join("link.bin")).unwrap();
        let link = dir.path().join("link.bin");
        assert!(!same_blob(&link, hash, body.len() as u64).unwrap());
    }

    /// A FIFO that appeared where a file was checked is refused at the open, and not waited on for a
    /// writer: `NONBLOCK` is what makes the refusal immediate, and the check of the opened handle is
    /// what makes it a regular file's worth of bytes or nothing.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_fifo_is_refused_instead_of_waited_on() {
        use super::open_no_follow;
        use rustix::fs::{CWD, Mode, mkfifoat};

        let dir = tempfile::tempdir().unwrap();
        let pipe = dir.path().join("pipe");
        mkfifoat(CWD, &pipe, Mode::RUSR | Mode::WUSR).unwrap();
        let err = open_no_follow(&pipe).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput, "{err}");
    }
}
