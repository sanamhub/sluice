//! Writing a checked folder: folders first, then each file from its blob or its pack, then the
//! links the user allowed, then the folder times, so creating a child does not overwrite them.

use std::{collections::BTreeSet, fs, path::Path};

use iroh_blobs::{
    Hash,
    api::blobs::{BlobReader, ExportMode, ExportOptions},
    store::fs::FsStore,
};
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio::sync::mpsc::UnboundedSender;

use super::{ReceiveOptions, fetch, plan::Plan, storage};
use crate::{
    Error,
    engine::{Event, emit},
    manifest::{Kind, Loc, Manifest},
};

pub(super) async fn write(
    manifest: &Manifest,
    plan: Plan,
    store: &FsStore,
    conn: &iroh::endpoint::Connection,
    options: &ReceiveOptions,
    events: &UnboundedSender<Event>,
) -> Result<(), Error> {
    let dirs = create_dirs(manifest, options)?;
    let mut done = 0u64;
    let mut packs = BTreeSet::new();
    for entry in manifest.entries() {
        if entry.kind != Kind::File {
            continue;
        }
        match entry.loc {
            Loc::Blob { root } => {
                let target = options.out.join(entry.path.as_str());
                write_blob(
                    store,
                    conn,
                    Hash::from(root),
                    &target,
                    entry.size,
                    entry.mtime_ms,
                    entry.mode,
                    options,
                    events,
                )
                .await?;
            }
            Loc::Packed { pack_index, .. } => {
                if !packs.insert(pack_index) {
                    continue;
                }
                let pack_bytes =
                    write_pack(manifest, pack_index as usize, store, conn, options, events).await?;
                done = done.saturating_add(pack_bytes);
                emit(
                    events,
                    Event::Progress {
                        bytes: done,
                        total: plan.bytes,
                    },
                );
                continue;
            }
            Loc::None => {
                return Err(Error::Bug(
                    "a File entry has no location to write".to_string(),
                ));
            }
        }
        done = done.saturating_add(entry.size);
        emit(
            events,
            Event::Progress {
                bytes: done,
                total: plan.bytes,
            },
        );
    }
    if options.links {
        for entry in manifest.entries() {
            if entry.kind == Kind::Symlink {
                write_link(entry, options)?;
            }
        }
    }
    for (target, mtime_ms, mode) in dirs.iter().rev() {
        set_mode(target, *mode)?;
        if !options.no_times {
            set_time(target, *mtime_ms)?;
        }
    }
    Ok(())
}

fn create_dirs(
    manifest: &Manifest,
    options: &ReceiveOptions,
) -> Result<Vec<(std::path::PathBuf, u64, u32)>, Error> {
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

/// Writes every file of one pack and returns the bytes it wrote. The pack is fetched once.
async fn write_pack(
    manifest: &Manifest,
    pack_index: usize,
    store: &FsStore,
    conn: &iroh::endpoint::Connection,
    options: &ReceiveOptions,
    events: &UnboundedSender<Event>,
) -> Result<u64, Error> {
    let pack = manifest.packs().get(pack_index).ok_or_else(|| {
        Error::Policy("the manifest points at a pack that is not listed".to_string())
    })?;
    fetch::fetch_blob(store, conn, Hash::from(pack.root), 0, events).await?;
    let (_, size) = fetch::complete_bytes(store, Hash::from(pack.root)).await?;
    if size != pack.size {
        return Err(Error::Verify(format!(
            "a pack has {size} bytes and the manifest says {}",
            pack.size
        )));
    }
    let mut reader = store.blobs().reader(Hash::from(pack.root));
    let mut written = 0u64;
    for entry in manifest.entries() {
        let Loc::Packed {
            pack_index: other,
            offset,
            len,
        } = entry.loc
        else {
            continue;
        };
        if other as usize != pack_index {
            continue;
        }
        let target = options.out.join(entry.path.as_str());
        write_from_pack(
            &mut reader,
            offset,
            len,
            &target,
            entry.mtime_ms,
            entry.mode,
            options.no_times,
        )
        .await?;
        written = written.saturating_add(len);
    }
    Ok(written)
}

async fn write_from_pack(
    reader: &mut BlobReader,
    offset: u64,
    len: u64,
    target: &Path,
    mtime_ms: u64,
    mode: u32,
    no_times: bool,
) -> Result<(), Error> {
    let incoming = storage::incoming_path(target);
    let mut file = tokio::fs::File::create(&incoming)
        .await
        .map_err(|e| storage::storage("create", &incoming, e))?;
    reader
        .seek(std::io::SeekFrom::Start(offset))
        .await
        .map_err(|e| storage::storage("read a pack", target, e))?;
    let copied = tokio::io::copy(&mut reader.take(len), &mut file)
        .await
        .map_err(|e| storage::storage("write", target, e))?;
    file.sync_all()
        .await
        .map_err(|e| storage::storage("sync", &incoming, e))?;
    drop(file);
    if copied != len {
        return Err(Error::Verify(format!(
            "a packed file has {copied} bytes and the manifest says {len}"
        )));
    }
    storage::install(&incoming, target, true)?;
    finish_file(target, mtime_ms, mode, no_times)
}

#[allow(clippy::too_many_arguments)]
async fn write_blob(
    store: &FsStore,
    conn: &iroh::endpoint::Connection,
    hash: Hash,
    target: &Path,
    size: u64,
    mtime_ms: u64,
    mode: u32,
    options: &ReceiveOptions,
    events: &UnboundedSender<Event>,
) -> Result<(), Error> {
    let incoming = storage::incoming_path(target);
    if size == 0 {
        fs::write(&incoming, []).map_err(|e| storage::storage("create", target, e))?;
    } else {
        fetch::fetch_blob(store, conn, hash, 0, events).await?;
        let (_, got) = fetch::complete_bytes(store, hash).await?;
        if got != size {
            return Err(Error::Verify(format!(
                "a blob has {got} bytes and the manifest says {size}"
            )));
        }
        store
            .blobs()
            .export_with_opts(ExportOptions {
                hash,
                mode: ExportMode::TryReference,
                target: incoming.clone(),
            })
            .finish()
            .await
            .map_err(|e| Error::Storage(format!("cannot write {}: {e}", target.display())))?;
    }
    storage::sync_file(&incoming)?;
    storage::install(&incoming, target, options.force)?;
    finish_file(target, mtime_ms, mode, options.no_times)
}

fn finish_file(target: &Path, mtime_ms: u64, mode: u32, no_times: bool) -> Result<(), Error> {
    if !no_times {
        set_time(target, mtime_ms)?;
    }
    set_mode(target, mode)
}

#[cfg(unix)]
fn write_link(entry: &crate::manifest::Entry, options: &ReceiveOptions) -> Result<(), Error> {
    let Some(target) = entry.link_target.as_ref().map(|target| target.as_str()) else {
        return Err(Error::Bug("a symlink entry has no target".to_string()));
    };
    let link = options.out.join(entry.path.as_str());
    let parent = link
        .parent()
        .ok_or_else(|| Error::Bug("a link has no parent".to_string()))?;
    let real_out = fs::canonicalize(&options.out)
        .map_err(|e| storage::policy_path("resolve", &options.out, e))?;
    let real_parent =
        fs::canonicalize(parent).map_err(|e| storage::policy_path("resolve", parent, e))?;
    if !real_parent.starts_with(&real_out) {
        return Err(Error::Policy(
            "the link would be created outside the output folder".to_string(),
        ));
    }
    if let Ok(real) = fs::canonicalize(real_parent.join(target))
        && !real.starts_with(&real_out)
    {
        return Err(Error::Policy(
            "the link points outside the output folder".to_string(),
        ));
    }
    std::os::unix::fs::symlink(target, &link)
        .map_err(|e| storage::policy_path("create a symlink at", &link, e))
}

#[cfg(windows)]
fn write_link(entry: &crate::manifest::Entry, options: &ReceiveOptions) -> Result<(), Error> {
    let Some(target) = entry.link_target.as_ref().map(|target| target.as_str()) else {
        return Err(Error::Bug("a symlink entry has no target".to_string()));
    };
    let link = options.out.join(entry.path.as_str());
    std::os::windows::fs::symlink_file(target, &link).map_err(|e| {
        Error::Policy(format!(
            "cannot create a symlink at {}: {e}; Windows needs developer mode or an elevated \
             process for links",
            link.display()
        ))
    })
}

#[cfg(not(any(unix, windows)))]
fn write_link(_entry: &crate::manifest::Entry, _options: &ReceiveOptions) -> Result<(), Error> {
    Err(Error::Policy(
        "this platform has no symlink call".to_string(),
    ))
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> Result<(), Error> {
    use std::os::unix::fs::PermissionsExt;
    if mode == 0 {
        return Ok(());
    }
    // Setuid and setgid are never applied, whatever the sender wrote.
    let mode = mode & 0o777;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .map_err(|e| storage::policy_path("set the mode of", path, e))
}

#[cfg(windows)]
fn set_mode(path: &Path, mode: u32) -> Result<(), Error> {
    if mode == 0 {
        return Ok(());
    }
    let mut permissions = fs::metadata(path)
        .map_err(|e| storage::policy_path("read the mode of", path, e))?
        .permissions();
    permissions.set_readonly(mode & 0o200 == 0);
    fs::set_permissions(path, permissions)
        .map_err(|e| storage::policy_path("set the mode of", path, e))
}

#[cfg(not(any(unix, windows)))]
fn set_mode(_path: &Path, _mode: u32) -> Result<(), Error> {
    Ok(())
}

#[cfg(unix)]
fn set_time(path: &Path, mtime_ms: u64) -> Result<(), Error> {
    if mtime_ms == 0 {
        return Ok(());
    }
    let time = std::time::UNIX_EPOCH + std::time::Duration::from_millis(mtime_ms);
    // A folder is opened read-only; a file needs write access on Windows, and this branch is Unix.
    let file = fs::File::open(path).map_err(|e| storage::storage("open", path, e))?;
    file.set_times(fs::FileTimes::new().set_modified(time))
        .map_err(|e| storage::storage("set the time of", path, e))
}

#[cfg(windows)]
fn set_time(path: &Path, mtime_ms: u64) -> Result<(), Error> {
    if mtime_ms == 0 || fs::metadata(path).is_ok_and(|meta| meta.is_dir()) {
        return Ok(());
    }
    let time = std::time::UNIX_EPOCH + std::time::Duration::from_millis(mtime_ms);
    let file = fs::File::options()
        .write(true)
        .open(path)
        .map_err(|e| storage::storage("open", path, e))?;
    file.set_times(fs::FileTimes::new().set_modified(time))
        .map_err(|e| storage::storage("set the time of", path, e))
}

#[cfg(not(any(unix, windows)))]
fn set_time(_path: &Path, _mtime_ms: u64) -> Result<(), Error> {
    Ok(())
}
