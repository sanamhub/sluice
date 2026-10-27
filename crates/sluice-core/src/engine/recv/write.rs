//! Writing a checked tree: folders first, then each file from its blob or its pack, then the links
//! the user allowed, then the folder times, so creating a child does not overwrite them.

mod file;
mod link;

use std::{collections::BTreeSet, fs, path::PathBuf};

use iroh_blobs::{Hash, api::blobs::ExportMode, store::fs::FsStore};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio::sync::mpsc::UnboundedSender;

use super::{Event, ReceiveOptions, fetch, plan::Plan, storage};
use crate::{
    Error,
    manifest::{Kind, Loc, Manifest},
};

use file::Meta;

pub(super) async fn write(
    manifest: &Manifest,
    _plan: Plan,
    store: &FsStore,
    options: &ReceiveOptions,
    resume: bool,
    events: &UnboundedSender<Event>,
) -> Result<(), Error> {
    let dirs = create_dirs(manifest, options)?;
    let tmp = Temp(storage::temp_dir(&options.out)?);
    let mut writer = Writer {
        store,
        options,
        resume,
        tmp: &tmp.0,
        next: 0,
        events,
    };
    writer.files(manifest).await?;
    if options.links {
        for entry in manifest.entries() {
            if entry.kind == Kind::Symlink {
                link::write(entry, options, resume)?;
            }
        }
    }
    for (target, mtime_ms, mode) in dirs.iter().rev() {
        file::set_mode_path(target, *mode)?;
        if !options.no_times {
            file::set_time_path(target, *mtime_ms, events)?;
        }
    }
    Ok(())
}

/// The temporary folder, removed however the write ends.
struct Temp(PathBuf);

impl Drop for Temp {
    fn drop(&mut self) {
        storage::remove_quietly(&self.0);
    }
}

struct Writer<'a> {
    store: &'a FsStore,
    options: &'a ReceiveOptions,
    resume: bool,
    tmp: &'a std::path::Path,
    next: u64,
    events: &'a UnboundedSender<Event>,
}

impl Writer<'_> {
    /// Every file, in manifest order. One pack is written the first time an entry names it.
    async fn files(&mut self, manifest: &Manifest) -> Result<(), Error> {
        let mut packs = BTreeSet::new();
        for entry in manifest.entries() {
            if entry.kind != Kind::File {
                continue;
            }
            match entry.loc {
                Loc::Blob { root } => self.blob(entry, Hash::from(root)).await?,
                Loc::Packed { pack_index, .. } => {
                    if packs.insert(pack_index) {
                        self.pack(manifest, pack_index as usize).await?;
                    }
                }
                Loc::None => {
                    return Err(Error::Bug(
                        "a File entry has no location to write".to_string(),
                    ));
                }
            }
        }
        Ok(())
    }

    /// A name that no entry can collide with: the folder is new and only this writer names in it.
    fn fresh(&mut self) -> PathBuf {
        let path = storage::temp_path(self.tmp, self.next);
        self.next += 1;
        path
    }

    fn meta(entry: &crate::manifest::Entry) -> Meta {
        Meta {
            mtime_ms: entry.mtime_ms,
            mode: entry.mode,
        }
    }

    async fn blob(&mut self, entry: &crate::manifest::Entry, hash: Hash) -> Result<(), Error> {
        let target = self.options.out.join(entry.path.as_str());
        let meta = Self::meta(entry);
        if target.exists() && !self.options.force {
            if self.resume && file::same_blob(&target, hash, entry.size)? {
                return file::touch(&target, meta, self.options.no_times, self.events);
            }
            return Err(storage::clash(&target));
        }
        let size = fetch::complete_size(self.store, hash).await?;
        if size != entry.size {
            return Err(Error::Verify(format!(
                "a blob has {size} bytes and the manifest says {}",
                entry.size
            )));
        }
        let incoming = self.fresh();
        if size == 0 {
            file::create_empty(&incoming)?;
        } else {
            self.store
                .blobs()
                .export_with_opts(iroh_blobs::api::blobs::ExportOptions {
                    hash,
                    mode: ExportMode::TryReference,
                    target: incoming.clone(),
                })
                .finish()
                .await
                .map_err(|e| {
                    Error::Storage(format!(
                        "cannot write {}: {}",
                        target.display(),
                        crate::chain(&e)
                    ))
                })?;
        }
        file::finish(&incoming, meta, self.options.no_times, self.events)?;
        storage::install(&incoming, &target, self.options.force)
    }

    /// Writes every file of one pack. The pack was fetched and checked before anything was written.
    async fn pack(&mut self, manifest: &Manifest, index: usize) -> Result<(), Error> {
        let pack = manifest.packs().get(index).ok_or_else(|| {
            Error::Policy("the manifest points at a pack that is not listed".to_string())
        })?;
        let mut reader = self.store.blobs().reader(Hash::from(pack.root));
        for entry in manifest.entries() {
            let Loc::Packed { pack_index, .. } = entry.loc else {
                continue;
            };
            if pack_index as usize != index {
                continue;
            }
            let target = self.options.out.join(entry.path.as_str());
            let meta = Self::meta(entry);
            if target.exists() && !self.options.force {
                if self.resume && file::same_packed(&mut reader, entry, &target).await? {
                    file::touch(&target, meta, self.options.no_times, self.events)?;
                    continue;
                }
                return Err(storage::clash(&target));
            }
            let incoming = self.fresh();
            let file = file::copy_from_pack(&mut reader, entry, &incoming).await?;
            file::apply(&file, &incoming, meta, self.options.no_times, self.events)?;
            file.sync_all()
                .map_err(|e| storage::storage("sync", &incoming, e))?;
            drop(file);
            storage::install(&incoming, &target, self.options.force)?;
        }
        Ok(())
    }
}

/// Writes `len` bytes at `offset` of a blob in the store into `target`. The fetch that put those
/// blocks in the store verified them against the root, so this only copies and installs them.
#[allow(clippy::too_many_arguments)]
pub(in crate::engine) async fn write_range(
    store: &FsStore,
    hash: Hash,
    offset: u64,
    len: u64,
    target: &std::path::Path,
    mtime_ms: u64,
    mode: u32,
    options: &ReceiveOptions,
    events: &UnboundedSender<Event>,
) -> Result<(), Error> {
    let meta = Meta { mtime_ms, mode };
    let tmp = Temp(storage::temp_dir(&options.out)?);
    let incoming = storage::temp_path(&tmp.0, 0);
    let mut reader = store.blobs().reader(hash);
    reader
        .seek(std::io::SeekFrom::Start(offset))
        .await
        .map_err(|e| storage::storage("read", target, e))?;
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&incoming)
        .await
        .map_err(|e| storage::storage("create", &incoming, e))?;
    let copied = tokio::io::copy(&mut reader.take(len), &mut file)
        .await
        .map_err(|e| storage::storage("write", &incoming, e))?;
    file.flush()
        .await
        .map_err(|e| storage::storage("write", &incoming, e))?;
    let file = file.into_std().await;
    if copied != len {
        return Err(Error::Verify(format!(
            "the store gave {copied} bytes where the manifest says {len}"
        )));
    }
    file::apply(&file, &incoming, meta, options.no_times, events)?;
    file.sync_all()
        .map_err(|e| storage::storage("sync", &incoming, e))?;
    drop(file);
    storage::install(&incoming, target, options.force)
}
fn create_dirs(
    manifest: &Manifest,
    options: &ReceiveOptions,
) -> Result<Vec<(PathBuf, u64, u32)>, Error> {
    let mut dirs = Vec::new();
    for entry in manifest.entries() {
        if entry.kind == Kind::Dir {
            let target = options.out.join(entry.path.as_str());
            fs::create_dir_all(&target).map_err(|e| storage::policy_path("create", &target, e))?;
            dirs.push((target, entry.mtime_ms, entry.mode));
        }
    }
    Ok(dirs)
}
