//! One file from the sender's side: import it where it lives, build the one-entry manifest, and
//! prepare the ticket. The bytes are never copied into the home.

use std::{
    fs,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use iroh_blobs::{
    BlobFormat, Hash,
    api::{
        TempTag,
        blobs::{AddPathOptions, ImportMode},
    },
    store::fs::FsStore,
};
use tokio::sync::mpsc::UnboundedSender;

use super::{Congestion, Event, Provider, RelayModeChoice, Stop, emit, expiry, storage};
use crate::{
    Error, Home,
    manifest::{Entry, Kind, Loc, Manifest, PackRef, path::ManifestPath},
    ticket::{Body, Format, Ticket},
};

/// A file that is imported and ready to serve, with the ticket that names it.
pub struct Outgoing {
    /// The listening provider.
    pub provider: Provider,
    /// The ticket to print. Its secret is why this value is never logged.
    pub ticket: Ticket,
    /// The file's BLAKE3 root, for the one-download stop.
    pub root: [u8; 32],
    /// The file name the receiver will write.
    pub name: String,
    /// The congestion controller this send runs with.
    pub congestion: Congestion,
    // Keeps the in-place import protected from garbage collection while the send runs.
    _tag: TempTag,
}

/// Imports `path` in place and starts a provider for it. `expiry` is UTC milliseconds or none.
///
/// Fails with [`Error::Policy`] for a folder, a symlink or a name a receiver would reject, and with
/// [`Error::Verify`] if the file changes between the import and the start of serving.
pub async fn prepare(
    home: &Home,
    path: &Path,
    relay: RelayModeChoice,
    congestion: Congestion,
    expiry_ms: Option<u64>,
) -> Result<Outgoing, Error> {
    let shown = path.display().to_string();
    let meta = fs::symlink_metadata(path).map_err(|e| storage::policy_path("read", path, e))?;
    if meta.file_type().is_symlink() {
        return Err(Error::Policy(format!(
            "{shown} is a symlink; send the file it points at by its real path"
        )));
    }
    if !meta.is_file() {
        return Err(Error::Policy(format!(
            "{shown} is not a regular file; folders are added in a later change"
        )));
    }
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| Error::Policy(format!("{shown} has no UTF-8 file name")))?;
    let entry_path = ManifestPath::parse(name)?;
    let before = Stamp::of(path)?;

    let store = FsStore::load(home.path().join("blobs"))
        .await
        .map_err(|e| Error::Storage(format!("cannot open the store: {e}")))?;
    let tag = store
        .blobs()
        .add_path_with_opts(AddPathOptions {
            path: path.to_path_buf(),
            format: BlobFormat::Raw,
            mode: ImportMode::TryReference,
        })
        .temp_tag()
        .await
        .map_err(|e| Error::Storage(format!("cannot import {shown}: {e}")))?;
    let hash = tag.hash();
    if !before.same(path)? {
        return Err(Error::Verify(format!(
            "{shown} changed while it was read, so its root names no version of it"
        )));
    }

    let (packs, loc) = if before.size > 0 && before.size < crate::manifest::pack::PACK_THRESHOLD {
        (
            vec![PackRef {
                root: *hash.as_bytes(),
                size: before.size,
            }],
            Loc::Packed {
                pack_index: 0,
                offset: 0,
                len: before.size,
            },
        )
    } else {
        (
            vec![],
            Loc::Blob {
                root: *hash.as_bytes(),
            },
        )
    };
    let manifest = Manifest::new(
        vec![Entry {
            path: entry_path,
            kind: Kind::File,
            size: before.size,
            mtime_ms: before.mtime_ms,
            mode: before.mode,
            loc,
            link_target: None,
        }],
        packs,
    )?;
    let bytes = manifest.encode();
    let added = store
        .blobs()
        .add_bytes(bytes)
        .await
        .map_err(|e| Error::Storage(format!("cannot keep the manifest: {e}")))?;
    if added.hash != Hash::from(manifest.root()) {
        return Err(Error::Bug(
            "the store gave the manifest a root other than its BLAKE3 hash".to_string(),
        ));
    }

    let allowed = [manifest.root(), *hash.as_bytes()].into_iter().collect();
    let relay_url = match &relay {
        RelayModeChoice::Disabled => None,
        RelayModeChoice::Url(url) => Some(url.clone()),
        RelayModeChoice::N0 => None,
    };
    let provider = Provider::start(home, store, allowed, relay, congestion, expiry_ms).await?;
    let addr = provider.addr();
    let relay_url = relay_url.or_else(|| {
        addr.relay_urls()
            .next()
            .map(std::string::ToString::to_string)
    });
    let ticket = Ticket::new(
        *addr.id.as_bytes(),
        addr.ip_addrs().copied().collect(),
        relay_url,
        expiry_ms,
        Body::Static {
            root: manifest.root(),
            size: before.size,
            format: Format::HashSequence,
        },
    )?;
    // The ticket is readable by anyone who has it, so it is printed and never traced.
    Ok(Outgoing {
        provider,
        ticket,
        root: *hash.as_bytes(),
        name: name.to_string(),
        congestion,
        _tag: tag,
    })
}

impl Outgoing {
    /// Serves once, or until Ctrl+C when `keep`. Emits `started`, then a progress line is the
    /// provider's business; the sender does not count bytes.
    pub async fn serve(
        self,
        keep: bool,
        events: &UnboundedSender<Event>,
    ) -> Result<StopEnd, Error> {
        let bytes = self.ticket_size();
        emit(
            events,
            Event::Started {
                name: Some(self.name),
                files: 1,
                bytes,
                congestion: self.congestion,
            },
        );
        let end = self
            .provider
            .serve(if keep {
                Stop::Keep
            } else {
                Stop::OneDownload(self.root)
            })
            .await?;
        Ok(match end {
            super::ServeEnd::Downloaded => StopEnd::Downloaded,
            super::ServeEnd::Expired => StopEnd::Expired,
            super::ServeEnd::Interrupted => StopEnd::Interrupted,
        })
    }

    fn ticket_size(&self) -> u64 {
        match self.ticket.body() {
            Body::Static { size, .. } => *size,
            Body::Live { .. } => 0,
        }
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
#[allow(dead_code)] // Only the constructors of the current platform run.
enum FileId {
    Unix(u64, u64),
    Windows(u64, u64),
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
