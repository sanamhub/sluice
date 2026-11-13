//! The file a receiver already has and offers as the source of a delta: the one the user names with
//! `--delta-from`, and nothing else.
//!
//! It is opened as an input to a send is: a link is refused at the name and, with the open flag the
//! platform has for it, at the open itself, and what is opened must be a regular file that is still
//! the file the check saw. It is opened read-only and never written. It is read only inside the
//! length it had when it was opened, so a file that grows or shrinks while it is read is cut at that
//! length, and what a plan copies from it is bounded by the same number. If it changes, the file
//! that is assembled from it fails the check against the root, and the plain fetch runs.

use std::{
    fs::{self, File},
    io,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use sluice_delta::{Candidate as Chunks, Entry, Params, StreamCut};

use super::{BLOCK, CANDIDATE_ENTRIES};
use crate::engine::send::Stamp;
use crate::manifest::{Entry as ManifestEntry, Kind, Manifest};

/// A stop flag that is set when this is dropped, so a cut stops on every way out of the code that
/// started it.
#[derive(Default)]
pub(super) struct StopOnDrop(pub(super) Arc<AtomicBool>);

impl Drop for StopOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

/// An opened candidate: the handle, the length it had at the open, and what the file looked like
/// then, so a later look can tell whether it is still that file.
#[derive(Clone)]
pub(in crate::engine) struct Candidate {
    file: Arc<File>,
    len: u64,
    stamp: Stamp,
    path: PathBuf,
}

/// Where the candidate for `entry` is, under the path the user named. A folder gives the file at
/// the entry's own path inside it, with no link on the way; a file is the candidate of a transfer
/// that is exactly one file. `None` is no candidate for this entry, which is not an error.
pub(in crate::engine) fn candidate_path(
    from: &Path,
    manifest: &Manifest,
    entry: &ManifestEntry,
) -> Option<PathBuf> {
    let meta = fs::metadata(from).ok()?;
    if meta.is_dir() {
        let path = entry.path.under(from);
        let mut folder = path.parent();
        while let Some(at) = folder.filter(|at| at.starts_with(from) && *at != from) {
            if fs::symlink_metadata(at).ok()?.file_type().is_symlink() {
                return None;
            }
            folder = at.parent();
        }
        return Some(path);
    }
    let single = matches!(manifest.entries(), [only] if only.kind == Kind::File);
    (single && meta.is_file()).then(|| from.to_path_buf())
}

impl Candidate {
    /// Opens `path` as a candidate on a thread that may block. A path swapped for a pipe between the
    /// check and the open would hold an async worker for ever; it holds this thread, and the open
    /// itself does not wait for a writer (`O_NONBLOCK` on Unix).
    pub(in crate::engine) async fn open(path: &Path) -> Option<Self> {
        let path = path.to_path_buf();
        tokio::task::spawn_blocking(move || Self::open_blocking(&path))
            .await
            .ok()?
    }

    /// [`Self::open`] on the calling thread. `None` is a path that is not a regular file, is a
    /// link, cannot be read, or changed between the check and the open.
    pub(in crate::engine) fn open_blocking(path: &Path) -> Option<Self> {
        let meta = fs::symlink_metadata(path).ok()?;
        if meta.file_type().is_symlink() || !meta.is_file() {
            return None;
        }
        let stamp = Stamp::of(path).ok()?;
        let file = open_without_following(path).ok()?;
        let opened = file.metadata().ok()?;
        if !opened.is_file() || opened.len() != stamp.size || !stamp.same(path).ok()? {
            return None;
        }
        Some(Self {
            file: Arc::new(file),
            len: opened.len(),
            stamp,
            path: path.to_path_buf(),
        })
    }

    /// Whether the path still names the file that was opened, unchanged.
    pub(in crate::engine) fn unchanged(&self) -> bool {
        self.stamp.same(&self.path).unwrap_or(false)
    }

    /// Cuts the file with `params`, a block at a time, and keeps one entry per chunk up to
    /// [`CANDIDATE_ENTRIES`]. Past that the rest of the file is not read. A file that shrank while
    /// it was read is cut where it ended.
    pub(in crate::engine) async fn chunks(&self, params: Params) -> Option<Chunks> {
        let stop = StopOnDrop::default();
        self.chunks_until(params, &stop.0, std::time::Duration::ZERO)
            .await
    }

    /// [`Self::chunks`] that stops, at the next block, once `stop` is set: a cut that nobody is
    /// waiting for any more must not go on holding the file open. `delay` is what a test makes it
    /// wait for each block.
    pub(in crate::engine) async fn chunks_until(
        &self,
        params: Params,
        stop: &Arc<AtomicBool>,
        delay: std::time::Duration,
    ) -> Option<Chunks> {
        let file = Arc::clone(&self.file);
        let stop = Arc::clone(stop);
        let len = self.len;
        tokio::task::spawn_blocking(move || cut(&file, len, params, &stop, delay))
            .await
            .ok()?
            .ok()
    }

    /// How many handles hold the file open: this one, and each cut still running. A test only
    /// reaches this.
    #[cfg(test)]
    pub(in crate::engine) fn handles(&self) -> usize {
        Arc::strong_count(&self.file)
    }

    /// Fills `buf` from `offset`, which has to lie inside the length the file had at the open: a
    /// range past it is refused whatever the file holds now.
    pub(in crate::engine) fn read_exact_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        let end = offset.checked_add(buf.len() as u64);
        if end.is_none_or(|end| end > self.len) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "a read past the length the candidate had",
            ));
        }
        let got = read_full_at(&self.file, buf, offset)?;
        if got != buf.len() {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        Ok(())
    }
}

/// The chunks of `file`, read in blocks inside `len` bytes.
fn cut(
    file: &File,
    len: u64,
    params: Params,
    stop: &AtomicBool,
    delay: std::time::Duration,
) -> io::Result<Chunks> {
    let mut cutter = StreamCut::new(params);
    let mut entries: Vec<Entry> = Vec::new();
    let mut buffer = vec![0u8; BLOCK];
    let mut at = 0u64;
    let mut sound = true;
    while at < len && entries.len() < CANDIDATE_ENTRIES {
        if stop.load(Ordering::SeqCst) {
            return Err(io::ErrorKind::Interrupted.into());
        }
        if !delay.is_zero() {
            std::thread::sleep(delay);
        }
        let want = usize::try_from((len - at).min(BLOCK as u64)).unwrap_or(BLOCK);
        let got = read_full_at(file, &mut buffer[..want], at)?;
        if got == 0 {
            break;
        }
        at += got as u64;
        cutter.feed(&buffer[..got], |chunk| match Entry::of_bytes(chunk) {
            Ok(entry) => entries.push(entry),
            Err(_) => sound = false,
        });
        if got < want {
            break;
        }
    }
    if entries.len() < CANDIDATE_ENTRIES {
        cutter.finish(|chunk| match Entry::of_bytes(chunk) {
            Ok(entry) => entries.push(entry),
            Err(_) => sound = false,
        });
    }
    if !sound {
        return Err(io::Error::other("a chunk of the candidate has no length"));
    }
    entries.truncate(CANDIDATE_ENTRIES);
    Ok(Chunks::from_entries(&params, entries))
}

/// Reads until `buf` is full or the file ends, and says how many bytes that was.
fn read_full_at(file: &File, buf: &mut [u8], offset: u64) -> io::Result<usize> {
    let mut done = 0;
    while done < buf.len() {
        let n = read_at(file, &mut buf[done..], offset + done as u64)?;
        if n == 0 {
            break;
        }
        done += n;
    }
    Ok(done)
}

#[cfg(unix)]
fn read_at(file: &File, buf: &mut [u8], offset: u64) -> io::Result<usize> {
    std::os::unix::fs::FileExt::read_at(file, buf, offset)
}

#[cfg(windows)]
fn read_at(file: &File, buf: &mut [u8], offset: u64) -> io::Result<usize> {
    std::os::windows::fs::FileExt::seek_read(file, buf, offset)
}

/// Opens read-only, and where the platform has the flag, without following a link at the name.
#[cfg(unix)]
fn open_without_following(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(path)
}

#[cfg(windows)]
fn open_without_following(path: &Path) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    // A link is opened as the link and not its target, which `is_file` then refuses.
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    fs::OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
}

#[cfg(not(any(unix, windows)))]
fn open_without_following(path: &Path) -> io::Result<File> {
    fs::File::open(path)
}
