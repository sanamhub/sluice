//! One file from the sender's side: import it where it lives, build the one-entry manifest, and
//! prepare the ticket. The bytes are never copied into the home.

mod folder;
mod single;

pub use folder::prepare_paths;
pub(super) use single::content_roots;
pub use single::{prepare, prepare_stdin};

use std::{
    fs,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use iroh_blobs::api::TempTag;
use tokio::sync::mpsc::UnboundedSender;

use super::{Congestion, Event, Provider, RelayModeChoice, Stop, emit, expiry, storage};
use crate::{
    Error,
    ticket::{Body, Ticket},
};

/// A file that is imported and ready to serve, with the ticket that names it.
pub struct Outgoing {
    /// The listening provider.
    pub provider: Provider,
    /// The ticket to print. Its secret is why this value is never logged.
    pub ticket: Ticket,
    /// The content roots the one-download stop waits for.
    pub roots: Vec<[u8; 32]>,
    /// The file name when the transfer is one file, and `None` for a folder.
    pub name: Option<String>,
    /// How many files the receiver will write.
    pub files: u64,
    /// The sum of their sizes.
    pub bytes: u64,
    /// The congestion controller this send runs with.
    pub congestion: Congestion,
    // Keeps the in-place imports protected from garbage collection while the send runs.
    _tags: Vec<TempTag>,
}

/// Imports `path` in place and starts a provider for it. `expiry` is UTC milliseconds or none.
///
impl Outgoing {
    /// Serves once, or until Ctrl+C when `keep`. Emits `started`, then a progress line is the
    /// provider's business; the sender does not count bytes.
    pub async fn serve(
        self,
        keep: bool,
        events: &UnboundedSender<Event>,
    ) -> Result<StopEnd, Error> {
        emit(
            events,
            Event::Started {
                name: self.name.clone(),
                files: self.files,
                bytes: self.bytes,
                congestion: self.congestion,
            },
        );
        let roots = if self.roots.is_empty() {
            match self.ticket.body() {
                Body::Static { root, .. } => vec![*root],
                Body::Live { .. } => Vec::new(),
            }
        } else {
            self.roots.clone()
        };
        let end = self
            .provider
            .serve(if keep {
                Stop::Keep
            } else {
                Stop::OneDownload(roots)
            })
            .await?;
        Ok(match end {
            super::ServeEnd::Downloaded => StopEnd::Downloaded,
            super::ServeEnd::Expired => StopEnd::Expired,
            super::ServeEnd::Interrupted => StopEnd::Interrupted,
        })
    }
}

/// Why `send` returned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopEnd {
    /// One complete download finished.
    Downloaded,
    /// The ticket expiry stopped the provider.
    Expired,
    /// Ctrl+C.
    Interrupted,
}

/// What the sender records about a file before and after reading it.
#[derive(Clone, Copy, Debug)]
struct Stamp {
    size: u64,
    modified: Option<SystemTime>,
    id: FileId,
    mtime_ms: u64,
    mode: u32,
}

impl Stamp {
    fn of(path: &Path) -> Result<Self, Error> {
        let meta = fs::metadata(path).map_err(|e| storage::policy_path("read", path, e))?;
        let modified = meta.modified().ok();
        let mtime_ms = modified
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_millis() as u64);
        Ok(Self {
            size: meta.len(),
            modified,
            id: FileId::of(path, &meta)?,
            mtime_ms,
            mode: mode_of(&meta),
        })
    }

    fn same(&self, path: &Path) -> Result<bool, Error> {
        let now = Self::of(path)?;
        Ok(self.size == now.size
            && self.modified == now.modified
            && self.id == now.id
            && self.mtime_ms == now.mtime_ms
            && self.mode == now.mode)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FileId {
    #[cfg(unix)]
    Unix(u64, u64),
    #[cfg(windows)]
    Windows(u64, u64),
    #[cfg(not(any(unix, windows)))]
    Unknown,
}

impl FileId {
    #[cfg(unix)]
    fn of(_path: &Path, meta: &fs::Metadata) -> Result<Self, Error> {
        use std::os::unix::fs::MetadataExt;
        Ok(Self::Unix(meta.dev(), meta.ino()))
    }

    #[cfg(windows)]
    #[allow(unsafe_code)]
    fn of(path: &Path, _meta: &fs::Metadata) -> Result<Self, Error> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
        };
        let file = fs::File::open(path).map_err(|e| storage::policy_path("open", path, e))?;
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        // SAFETY: the handle is open for the call, and `info` is a valid out parameter.
        let done = unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) };
        if done == 0 {
            return Err(storage::policy_path(
                "read the file id of",
                path,
                std::io::Error::last_os_error(),
            ));
        }
        let index = (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow);
        Ok(Self::Windows(u64::from(info.dwVolumeSerialNumber), index))
    }

    #[cfg(not(any(unix, windows)))]
    fn of(_path: &Path, _meta: &fs::Metadata) -> Result<Self, Error> {
        Ok(Self::Unknown)
    }
}

#[cfg(unix)]
fn mode_of(meta: &fs::Metadata) -> u32 {
    use std::os::unix::fs::MetadataExt;
    meta.mode() & 0o7777
}

#[cfg(windows)]
fn mode_of(meta: &fs::Metadata) -> u32 {
    if meta.permissions().readonly() {
        0o444
    } else {
        0o644
    }
}

#[cfg(not(any(unix, windows)))]
fn mode_of(_meta: &fs::Metadata) -> u32 {
    0
}

/// The default expiry: 24 hours from now.
pub fn default_expiry(now_ms: u64) -> Option<u64> {
    expiry::deadline(now_ms, Some(expiry::DEFAULT_SECONDS))
}

#[cfg(test)]
mod tests {
    use super::default_expiry;

    #[test]
    fn the_default_is_one_day() {
        assert_eq!(default_expiry(1_000), Some(1_000 + 86_400_000));
    }
}
