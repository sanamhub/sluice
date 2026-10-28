//! The disk side of one receive: the space it needs, a private temporary folder, and the errors it
//! reports. The partial store itself is in [`store`], and installing a finished file in [`install`].

mod install;
mod meta;
mod registry;
mod store;
mod temp;

pub use install::install;
pub use meta::MetaStore;
#[cfg(test)]
pub(crate) use registry::remember_as;
pub use registry::{
    clash as registry_clash, discard, find as find_record, forget as forget_record,
    remember as remember_record,
};
#[cfg(test)]
pub use store::MARKER;
pub use store::{FolderId, Owner, claim, identify, partial_dir, recorded, remember};
pub use temp::{Temp, sweep_temporary_store, temp_dir, temp_path};

use std::{fs, io, path::Path, time::Duration};

use iroh_blobs::store::{
    GcConfig,
    fs::{FsStore, options::Options},
};

use crate::Error;

/// How much room a receive of `content` bytes needs before it starts. The store keeps the partial
/// blob and its outboard, about 0.4% of the content, and the manifest and metadata are small. A
/// finished file is moved out of the store rather than copied, so with hard links this is a peak and
/// not 2x. A volume that cannot link copies each file out while its blob is still in the store, so
/// there the whole content is held twice.
pub fn needed_bytes(content: u64, links: bool) -> u64 {
    const OUTBOARD_PARTS_PER_THOUSAND: u64 = 8;
    const SLACK: u64 = 1 << 20;
    let room = content.saturating_add(content / 1000 * OUTBOARD_PARTS_PER_THOUSAND);
    if links {
        room.saturating_add(SLACK)
    } else {
        room.saturating_mul(2).saturating_add(SLACK)
    }
}

/// How often the store's collector runs. A live stream drops each segment's tag as soon as the
/// segment is written out, and this is the longest the bytes can stay after that; it is also the
/// longest a segment is kept for a reader whose request is still on its way. Ten seconds is a few
/// segments at the rates a live stream is sealed at and far longer than a reader needs to have its
/// request handled.
pub const GC_INTERVAL: Duration = Duration::from_secs(10);

/// Whether a store runs the collector that frees a blob no tag holds.
///
/// Every caller states it, so a store cannot be opened without saying whether its blobs may be
/// freed. Only a store whose blobs are throwaway collects: the live send and the live receive keep
/// each segment while it is being served or written, and free it afterwards. Every other store is a
/// resume store, a pack a range reads, or a manifest a check depends on, and has to keep its bytes
/// until the store is done or discarded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Collector {
    /// A blob stays only while a tag or a temporary tag holds it.
    On,
    /// Every blob stays until the store is shut down or its folder is removed.
    Off,
}

/// Opens the store at `root` with the collector the caller chose.
pub async fn open_store(root: &Path, collector: Collector) -> Result<FsStore, Error> {
    let interval = match collector {
        Collector::On => Some(gc_interval()),
        Collector::Off => None,
    };
    let mut options = Options::new(root);
    options.gc = interval.map(|interval| GcConfig {
        interval,
        add_protected: None,
    });
    FsStore::load_with_opts(root.join("blobs.db"), options)
        .await
        .map_err(|e| Error::Storage(format!("cannot open the store at {}: {e}", root.display())))
}

/// How often a collector sweeps. A unit test sweeps sooner so it does not wait ten seconds for the
/// first sweep; a command always sweeps at [`GC_INTERVAL`].
#[cfg(test)]
fn gc_interval() -> Duration {
    Duration::from_millis(100)
}

#[cfg(not(test))]
fn gc_interval() -> Duration {
    GC_INTERVAL
}

/// [`open_store`] with the collector's interval as an argument, so a test can make one sweep
/// happen sooner than the constant does.
#[cfg(test)]
pub(super) async fn open_store_after(root: &Path, interval: Duration) -> Result<FsStore, Error> {
    let mut options = Options::new(root);
    options.gc = Some(GcConfig {
        interval,
        add_protected: None,
    });
    FsStore::load_with_opts(root.join("blobs.db"), options)
        .await
        .map_err(|e| Error::Storage(format!("cannot open the store at {}: {e}", root.display())))
}
/// Bytes free to this user on the volume that holds `path`.
pub fn available_bytes(path: &Path) -> Result<u64, Error> {
    imp::available(path).map_err(|e| {
        Error::Storage(format!(
            "cannot read the free space at {}: {e}",
            path.display()
        ))
    })
}

/// Whether the volume holding `dir` can make a hard link, which decides whether a finished file can
/// be moved out of the store or has to be copied out beside it. Probed rather than assumed: a
/// transfer that assumes links on a volume without them can fill the disk with the copy.
pub fn supports_links(dir: &Path) -> bool {
    let Ok(tmp) = temp_dir(dir) else {
        return false;
    };
    let from = temp_path(tmp.path(), 0);
    let to = temp_path(tmp.path(), 1);
    fs::write(&from, b"").is_ok() && fs::hard_link(&from, &to).is_ok()
}

/// Refuses when `dir` does not have room for a transfer of `content` bytes plus the store beside
/// it. The store is what a receive writes before it writes the file, so this is the volume that
/// has to hold both during the transfer.
pub fn ensure_space(dir: &Path, content: u64) -> Result<(), Error> {
    let available = available_bytes(dir)?;
    let needed = needed_bytes(content, true);
    if available < needed {
        return Err(Error::Storage(format!(
            "{} has {available} bytes free and this transfer needs about {needed}",
            dir.display()
        )));
    }
    Ok(())
}

/// The one clash message, naming the first path and both ways out.
pub fn clash(target: &Path) -> Error {
    Error::Policy(format!(
        "{} already exists; pass --force to replace it, or choose another --out",
        target.display()
    ))
}

pub fn storage(doing: &str, path: &Path, source: io::Error) -> Error {
    Error::Storage(format!("cannot {doing} {}: {source}", path.display()))
}

pub fn policy_path(doing: &str, path: &Path, source: io::Error) -> Error {
    Error::Policy(format!("cannot {doing} {}: {source}", path.display()))
}

/// Removes a file or folder that this receive made, ignoring a failure: the transfer itself
/// succeeded or failed for a reason worth reporting first.
pub fn remove_quietly(path: &Path) {
    let _ = fs::remove_dir_all(path);
    let _ = fs::remove_file(path);
}

#[cfg(unix)]
mod imp {
    use std::path::Path;

    pub(super) fn available(path: &Path) -> Result<u64, std::io::Error> {
        let stat = rustix::fs::statvfs(path)?;
        Ok(stat.f_bavail.saturating_mul(stat.f_frsize))
    }
}

#[cfg(windows)]
mod imp {
    use std::path::Path;

    pub(super) fn available(path: &Path) -> Result<u64, std::io::Error> {
        crate::home::private_windows::free_bytes(path)
    }
}

#[cfg(not(any(unix, windows)))]
mod imp {
    use std::path::Path;

    pub(super) fn available(_path: &Path) -> Result<u64, std::io::Error> {
        Err(std::io::Error::other("this platform has no call for it"))
    }
}

#[cfg(test)]
mod tests;
