//! Writing a checked tree: folders first, then each file from its blob or its pack, then the links
//! the user allowed, then the folder times, so creating a child does not overwrite them.

mod file;
mod link;

use std::{collections::BTreeMap, fs, path::PathBuf};

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
    partial: &std::path::Path,
    resume: bool,
    events: &UnboundedSender<Event>,
) -> Result<(), Error> {
    let counts = blob_counts(manifest);
    let dirs = create_dirs(manifest, options)?;
    let tmp = Temp(storage::temp_dir(&options.out)?);
    let mut writer = Writer {
        store,
        options,
        partial,
        resume,
        counts,
        write_limit: options.write_limit,
        tmp: &tmp.0,
        next: 0,
        events,
    };
    writer.files(manifest).await?;
    if options.links {
        link::write_all(manifest, options, resume)?;
    }
    for (target, mtime_ms, mode) in dirs.iter().rev() {
        file::set_mode_path(target, *mode)?;
        if !options.no_times {
            file::set_time_path(target, *mtime_ms, events)?;
        }
    }
    Ok(())
}

/// How many File entries name each blob root. One naming it can take the store's own file, because
/// nothing else needs it afterwards; several cannot, because the first would leave the store and
/// the rest with no bytes to read.
fn blob_counts(manifest: &Manifest) -> BTreeMap<[u8; 32], u64> {
    let mut counts = BTreeMap::new();
    for entry in manifest.entries() {
        if entry.kind == Kind::File
            && let Loc::Blob { root } = entry.loc
        {
            *counts.entry(root).or_insert(0) += 1;
        }
    }
    counts
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
    /// The per-transfer store folder, beside the output. Its data file is the cheapest copy.
    partial: &'a std::path::Path,
    resume: bool,
    counts: BTreeMap<[u8; 32], u64>,
    /// A test's full-disk hook: fail once this many bytes are written.
    write_limit: Option<u64>,
    tmp: &'a std::path::Path,
    next: u64,
    events: &'a UnboundedSender<Event>,
}

impl Writer<'_> {
    /// Every file, in manifest order. One pack is written the first time an entry names it.
    async fn files(&mut self, manifest: &Manifest) -> Result<(), Error> {
        let mut packs = std::collections::BTreeSet::new();
        for entry in manifest.entries() {
            if entry.kind != Kind::File {
                continue;
            }
            match entry.loc {
                Loc::Blob { root } => Box::pin(self.blob(entry, Hash::from(root))).await?,
                Loc::Packed { pack_index, .. } => {
                    if packs.insert(pack_index) {
                        Box::pin(self.pack(manifest, pack_index as usize)).await?;
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
        let target = entry.path.under(&self.options.out);
        let meta = Self::meta(entry);
        if self.already_there(&target, hash, entry.size)? {
            return file::touch(&target, meta, self.options.no_times, self.events);
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
            self.export(hash, &incoming).await?;
        }
        file::finish(&incoming, meta, self.options.no_times, self.events)?;
        storage::install(&incoming, &target, self.options.force)
    }

    /// Whether this target is a file whose bytes already match, so a resumed receive skips it. A
    /// link is never that file, however its target reads: it is a clash.
    fn already_there(
        &self,
        target: &std::path::Path,
        hash: Hash,
        size: u64,
    ) -> Result<bool, Error> {
        let meta = match fs::symlink_metadata(target) {
            Ok(meta) => meta,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(storage::policy_path("check", target, e)),
        };
        if self.options.force {
            return Ok(false);
        }
        if self.resume && meta.is_file() && file::same_blob(target, hash, size)? {
            return Ok(true);
        }
        Err(storage::clash(target))
    }

    /// Puts the blob's bytes at `incoming`. A root that one entry names takes the store's own data
    /// file by link, so the store keeps no second copy and one large file never needs twice its
    /// size; a root that several entries name is copied, because each target must stand alone.
    async fn export(&self, hash: Hash, incoming: &std::path::Path) -> Result<(), Error> {
        let once = self
            .counts
            .get(hash.as_bytes())
            .copied()
            .unwrap_or_default()
            == 1;
        if once {
            // The default store layout, which `receive` opens with `FsStore::load`.
            let source = iroh_blobs::store::fs::options::Options::new(self.partial)
                .path
                .data_path(&hash);
            // A hard link costs no space and leaves the store able to read the bytes it names.
            if fs::hard_link(&source, incoming).is_ok() {
                return Ok(());
            }
        }
        self.store
            .blobs()
            .export_with_opts(iroh_blobs::api::blobs::ExportOptions {
                hash,
                mode: ExportMode::Copy,
                target: incoming.to_path_buf(),
            })
            .finish()
            .await
            .map(|_| ())
            .map_err(|e| {
                Error::Storage(format!(
                    "cannot write {}: {}",
                    incoming.display(),
                    crate::plain(&e)
                ))
            })
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
            let target = entry.path.under(&self.options.out);
            let meta = Self::meta(entry);
            if self.packed_there(&mut reader, entry, &target).await? {
                file::touch(&target, meta, self.options.no_times, self.events)?;
                continue;
            }
            let incoming = self.fresh();
            let file =
                file::copy_from_pack(&mut reader, entry, &incoming, self.write_limit).await?;
            file::apply(&file, &incoming, meta, self.options.no_times, self.events)?;
            file.sync_all()
                .map_err(|e| storage::storage("sync", &incoming, e))?;
            drop(file);
            storage::install(&incoming, &target, self.options.force)?;
        }
        Ok(())
    }

    /// Whether a packed target is a file whose bytes already match. As in `already_there`, a link
    /// is a clash and never read.
    async fn packed_there(
        &self,
        reader: &mut iroh_blobs::api::blobs::BlobReader,
        entry: &crate::manifest::Entry,
        target: &std::path::Path,
    ) -> Result<bool, Error> {
        let meta = match fs::symlink_metadata(target) {
            Ok(meta) => meta,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(storage::policy_path("check", target, e)),
        };
        if self.options.force {
            return Ok(false);
        }
        if self.resume && meta.is_file() && file::same_packed(reader, entry, target).await? {
            return Ok(true);
        }
        Err(storage::clash(target))
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
            let target = entry.path.under(&options.out);
            fs::create_dir_all(&target).map_err(|e| storage::policy_path("create", &target, e))?;
            dirs.push((target, entry.mtime_ms, entry.mode));
        }
    }
    Ok(dirs)
}

#[cfg(test)]
mod tests;
