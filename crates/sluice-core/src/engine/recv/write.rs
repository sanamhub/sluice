//! Writing a checked tree: folders first, then each file from its blob or its pack, then the links
//! the user allowed, then the folder times, so creating a child does not overwrite them.

mod dirs;
mod file;
mod link;
mod range;
mod shared;

use std::{collections::BTreeMap, fs, path::PathBuf};

use iroh_blobs::{Hash, api::blobs::ExportMode, store::fs::FsStore};
use tokio::sync::mpsc::UnboundedSender;

use super::{Event, ReceiveOptions, fetch, plan::Plan, storage};
use crate::{
    Error,
    manifest::{Kind, Loc, Manifest},
};

use dirs::create_dirs;
use file::Meta;
pub(in crate::engine) use range::write_range;
use shared::Shared;

/// Writes the checked tree, and gives the store its own bytes back when a failure leaves it behind
/// for a resume: a file that took the store's bytes by link is the user's now, and the store must not
/// be changed through it.
pub(super) async fn write(
    manifest: &Manifest,
    plan: &Plan,
    store: &FsStore,
    options: &ReceiveOptions,
    partial: &std::path::Path,
    resume: bool,
    events: &UnboundedSender<Event>,
) -> Result<(), Error> {
    let mut shared = Shared::new(partial);
    let result = tree(
        manifest,
        plan,
        store,
        options,
        partial,
        resume,
        events,
        &mut shared,
    )
    .await;
    // The caller removes the store once every file is in place, and a refusal or a bad byte
    // discards it, so only a failure that keeps the store needs its bytes back. This is the one
    // place the copy happens: a cancelled command leaves the work to the store's next claim, which
    // is where it can be done without blocking the runtime.
    if let Err(error) = &result
        && super::keeps_store(error)
    {
        shared.release().await?;
    }
    result
}

/// The tree itself: folders, files, links and folder times.
#[allow(clippy::too_many_arguments)]
async fn tree(
    manifest: &Manifest,
    plan: &Plan,
    store: &FsStore,
    options: &ReceiveOptions,
    partial: &std::path::Path,
    resume: bool,
    events: &UnboundedSender<Event>,
    shared: &mut Shared,
) -> Result<(), Error> {
    let counts = blob_counts(manifest);
    let dirs = create_dirs(manifest, &options.out, &plan.ours)?;
    let tmp = storage::temp_dir(&options.out)?;
    let mut writer = Writer {
        store,
        options,
        partial,
        resume,
        counts,
        write_limit: options.write_limit,
        tmp: tmp.path(),
        next: 0,
        events,
        shared,
    };
    writer.files(manifest).await?;
    if options.links {
        link::write_all(manifest, options, resume)?;
    }
    for (target, mtime_ms, mode, made_here) in dirs.iter().rev() {
        // A folder that was already there keeps the mode and time it had: a resume writes into it
        // but does not take it over, and neither does a first run that met the user's own folder.
        if !made_here {
            continue;
        }
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

/// Whether the bytes written at `incoming` are the blob's. Every other export is the store's own
/// file or a copy of one nothing else can reach; this one came from a data file that another name
/// can change, so it is the written bytes that have to answer for the root.
fn check_written(incoming: &std::path::Path, hash: Hash, size: u64) -> Result<(), Error> {
    if file::same_blob(incoming, hash, size)? {
        return Ok(());
    }
    Err(Error::Verify(format!(
        "the blob written at {} changed while it was copied out, because a received file that \
         shares its bytes was edited; the store cannot be resumed from",
        incoming.display()
    )))
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
    /// The files that took the store's own bytes by link, so a store that is kept can be given a
    /// copy of each before a resume reads it.
    shared: &'a mut Shared,
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
        // A test's full-disk hook, for a blob as for a packed file: a file whose bytes would pass
        // the limit fails the way a full disk does, before anything is written.
        if let Some(limit) = self.write_limit
            && size > limit
        {
            return Err(Error::Storage(format!(
                "cannot write {}: no space left on device",
                target.display()
            )));
        }
        let linked = if size == 0 {
            file::create_empty(&incoming)?;
            false
        } else {
            self.export(hash, size, &incoming).await?
        };
        file::finish(&incoming, meta, self.options.no_times, self.events)?;
        storage::install(&incoming, &target, self.options.force)?;
        if linked {
            self.shared.took(hash, target);
        }
        Ok(())
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

    /// Puts the blob's bytes at `incoming`, and says whether they are the store's own file by link.
    /// A root that one entry names takes the store's data file by link, so the store keeps no second
    /// copy and one large file never needs twice its size; a root that several entries name is
    /// copied, because each target must stand alone. A stored blob that is linked to a file outside
    /// the store is hashed before it is used: the link is how a kept store and the output can share
    /// one inode, and an edit made through the output would otherwise be copied out as the blob.
    async fn export(
        &self,
        hash: Hash,
        size: u64,
        incoming: &std::path::Path,
    ) -> Result<bool, Error> {
        if !shared::store_holds(self.partial, hash, size)? {
            return Err(Error::Verify(
                "the store's copy of a blob no longer matches its root, because a received file \
                 that shares it was changed; the store cannot be resumed from"
                    .to_string(),
            ));
        }
        // A data file with a second name can be changed through that name, so its bytes are copied
        // out and the copy is what is checked. The check below cannot see an edit made after it,
        // and a link would export whatever the other name holds at the moment it is read.
        let reachable = shared::store_shares(self.partial, hash)?;
        let once = self
            .counts
            .get(hash.as_bytes())
            .copied()
            .unwrap_or_default()
            == 1;
        if once && !reachable {
            let source = storage::data_path(self.partial, &hash);
            // A hard link costs no space and leaves the store able to read the bytes it names.
            if fs::hard_link(&source, incoming).is_ok() {
                return Ok(true);
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
            .map_err(|e| {
                Error::Storage(format!(
                    "cannot write {}: {}",
                    incoming.display(),
                    crate::plain(&e)
                ))
            })?;
        if reachable {
            check_written(incoming, hash, size)?;
        }
        Ok(false)
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

#[cfg(test)]
mod tests;
