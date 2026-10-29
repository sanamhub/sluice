//! One file from the sender's side: import it where it lives, build the one-entry manifest, and
//! prepare the ticket. The bytes are never copied into the home.

mod folder;
mod scratch;
mod single;

pub(crate) use scratch::Scratch;

pub use folder::{prepare_paths, prepare_paths_with_rate};
pub(super) use single::content_roots;
#[cfg(test)]
pub(crate) use single::prepare_stdin_from;
pub use single::{prepare, prepare_stdin, prepare_stdin_with_rate, prepare_with_rate};

use std::{
    fs,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use iroh_blobs::api::TempTag;
use tokio::sync::mpsc::UnboundedSender;

use super::{Congestion, Event, LiveSend, Provider, RelayModeChoice, Stop, emit, expiry, storage};
use crate::{
    Error, Home,
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
    /// The log this send follows, when the content is live.
    pub live: Option<LiveSend>,
    // Keeps the in-place imports protected from garbage collection while the send runs.
    pub(crate) _tags: Vec<TempTag>,
}

#[cfg(test)]
impl Outgoing {
    /// The folder this send keeps its copies in, for a test that watches it while the send runs.
    pub(crate) fn scratch(&self) -> Option<&Path> {
        self.provider.scratch()
    }
}

/// How long a send serves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Keep {
    /// Until one receiver has taken the whole transfer.
    One,
    /// Until Ctrl+C or the ticket expiry.
    UntilStopped,
    /// Until this many distinct receivers have each taken the whole transfer.
    Receivers(u32),
}

/// Imports `path` in place and starts a provider for it. `expiry` is UTC milliseconds or none.
///
impl Outgoing {
    /// Claims the places this send offers, before its ticket is printed. `serve` sets the same
    /// limit again, but a ticket that is already known can be dialled before `serve` starts, and a
    /// request answered in that moment would be served without taking a place.
    pub fn reserve(&mut self, keep: Keep) {
        self.provider.set_limit(match keep {
            Keep::Receivers(limit) => Some(limit),
            Keep::One | Keep::UntilStopped => None,
        });
    }

    /// Serves until `keep` says to stop. Emits `started` and a line for every receiver; a progress
    /// line is the provider's business, and the sender does not count bytes.
    pub async fn serve(
        self,
        keep: Keep,
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
        let stop = match (&self.live, keep) {
            (_, Keep::UntilStopped) => Stop::Keep { limit: None, roots },
            (_, Keep::Receivers(limit)) => Stop::Keep {
                limit: Some(limit),
                roots,
            },
            (Some(live), Keep::One) => Stop::Live(live.log.clone()),
            (None, Keep::One) => Stop::OneDownload(roots),
        };
        let end = self.provider.serve(stop, events).await;
        if let Some(live) = &self.live {
            live.stop();
        }
        // The router shut the store down when it stopped serving and the provider removes the send's
        // folder after that, so a send that copied anything leaves nothing of it in the home.
        let end = end?;
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

/// Starts the provider for the store `scratch` opened, and gives it the folder: the provider closes
/// the store when it stops and removes the folder after that. A provider that could not start closes
/// the store itself and the folder goes with the failed send.
#[allow(clippy::too_many_arguments)]
pub(super) async fn serving(
    home: &Home,
    scratch: Scratch,
    allowed: std::collections::BTreeSet<[u8; 32]>,
    relay: RelayModeChoice,
    congestion: Congestion,
    expiry_ms: Option<u64>,
    port: Option<u16>,
    rate: Option<std::sync::Arc<super::Rate>>,
) -> Result<Provider, Error> {
    let provider = match Provider::start_with_rate(
        home,
        scratch.store().handle(),
        allowed,
        relay,
        congestion,
        expiry_ms,
        port,
        rate,
    )
    .await
    {
        Ok(provider) => provider,
        Err(e) => {
            let _ = scratch.close().await;
            return Err(e);
        }
    };
    serving_folder(provider, scratch)
}

/// [`serving`] for a live stream, whose provider serves the log's connections as well.
#[allow(clippy::too_many_arguments)]
pub(super) async fn serving_live(
    home: &Home,
    scratch: Scratch,
    log: std::sync::Arc<crate::engine::LiveLog>,
    relay: RelayModeChoice,
    congestion: Congestion,
    expiry_ms: Option<u64>,
    port: Option<u16>,
    rate: Option<std::sync::Arc<super::Rate>>,
) -> Result<Provider, Error> {
    let provider = match Provider::start_live_with_rate(
        home,
        scratch.store().handle(),
        log,
        relay,
        congestion,
        expiry_ms,
        port,
        rate,
    )
    .await
    {
        Ok(provider) => provider,
        Err(e) => {
            let _ = scratch.close().await;
            return Err(e);
        }
    };
    serving_folder(provider, scratch)
}

/// Hands the send's folder to a provider that has started, so it removes the folder once it has
/// closed the store.
fn serving_folder(provider: Provider, scratch: Scratch) -> Result<Provider, Error> {
    let mut provider = provider;
    let Some(dir) = scratch.release() else {
        return Err(Error::Bug("the send's folder is gone".to_string()));
    };
    provider.hold(dir);
    Ok(provider)
}
#[cfg(test)]
mod tests {
    use super::default_expiry;

    #[test]
    fn the_default_is_one_day() {
        assert_eq!(default_expiry(1_000), Some(1_000 + 86_400_000));
    }
}
