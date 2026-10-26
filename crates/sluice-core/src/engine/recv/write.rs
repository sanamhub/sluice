//! Writing a checked folder: folders first, then each file from its blob or its pack, then the
//! links the user allowed, then the folder times, so creating a child does not overwrite them.

use std::{collections::BTreeSet, fs, path::Path};

use iroh_blobs::{
    Hash,
    api::blobs::{BlobReader, ExportMode, ExportOptions},
    store::fs::FsStore,
};
use tokio::io::{AsyncReadExt, AsyncSeekExt};

use super::{ReceiveOptions, fetch, plan::Plan, storage};
use crate::{
    Error,
    manifest::{Kind, Loc, Manifest},
};

/// The metadata of one entry, kept together so the write helpers have fewer arguments.
#[derive(Clone, Copy)]
struct Meta {
    mtime_ms: u64,
    mode: u32,
}

pub(super) async fn write(
    manifest: &Manifest,
    _plan: Plan,
    store: &FsStore,
    options: &ReceiveOptions,
    resume: bool,
) -> Result<(), Error> {
    let dirs = create_dirs(manifest, options)?;
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
                    Hash::from(root),
                    &target,
                    entry.size,
                    Meta {
                        mtime_ms: entry.mtime_ms,
                        mode: entry.mode,
                    },
                    options,
                    resume,
                )
                .await?;
            }
            Loc::Packed { pack_index, .. } => {
                if !packs.insert(pack_index) {
                    continue;
                }
                write_pack(manifest, pack_index as usize, store, options, resume).await?;
                continue;
            }
            Loc::None => {
                return Err(Error::Bug(
                    "a File entry has no location to write".to_string(),
                ));
            }
        }
    }
    if options.links {
        for entry in manifest.entries() {
            if entry.kind == Kind::Symlink {
                write_link(entry, options, resume)?;
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

/// Writes every file of one pack. The pack was fetched and checked before anything was written.
async fn write_pack(
    manifest: &Manifest,
    pack_index: usize,
    store: &FsStore,
    options: &ReceiveOptions,
    resume: bool,
) -> Result<(), Error> {
    let pack = manifest.packs().get(pack_index).ok_or_else(|| {
        Error::Policy("the manifest points at a pack that is not listed".to_string())
    })?;
    let mut reader = store.blobs().reader(Hash::from(pack.root));
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
            Meta {
                mtime_ms: entry.mtime_ms,
                mode: entry.mode,
            },
            options,
            resume,
        )
        .await?;
    }
    Ok(())
}

async fn write_from_pack(
    reader: &mut BlobReader,
    offset: u64,
    len: u64,
    target: &Path,
    meta: Meta,
    options: &ReceiveOptions,
    resume: bool,
) -> Result<(), Error> {
    if target.exists() && !options.force {
        if resume && same_packed(reader, offset, len, target).await? {
            return finish_file(target, meta.mtime_ms, meta.mode, options.no_times);
        }
        return Err(storage::clash(target));
    }
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
    finish_file(target, meta.mtime_ms, meta.mode, options.no_times)
}

/// Whether a target already holds the bytes of this pack range, which is how a receive that was
/// killed after writing a file recognises it instead of refusing it as a clash.
async fn same_packed(
    reader: &mut BlobReader,
    offset: u64,
    len: u64,
    target: &Path,
) -> Result<bool, Error> {
    let existing = fs::read(target).map_err(|e| storage::storage("read", target, e))?;
    if existing.len() as u64 != len {
        return Ok(false);
    }
    reader
        .seek(std::io::SeekFrom::Start(offset))
        .await
        .map_err(|e| storage::storage("read a pack", target, e))?;
    let mut expected = vec![0u8; existing.len()];
    reader
        .read_exact(&mut expected)
        .await
        .map_err(|e| storage::storage("read a pack", target, e))?;
    Ok(existing == expected)
}

fn same_blob(target: &Path, hash: Hash, size: u64) -> Result<bool, Error> {
    let meta = fs::symlink_metadata(target).map_err(|e| storage::storage("read", target, e))?;
    if !meta.is_file() || meta.len() != size {
        return Ok(false);
    }
    if size == 0 {
        return Ok(true);
    }
    let mut file = fs::File::open(target).map_err(|e| storage::storage("read", target, e))?;
    let mut hasher = blake3::Hasher::new();
    std::io::copy(&mut file, &mut hasher).map_err(|e| storage::storage("read", target, e))?;
    Ok(*hasher.finalize().as_bytes() == *hash.as_bytes())
}

async fn write_blob(
    store: &FsStore,
    hash: Hash,
    target: &Path,
    size: u64,
    meta: Meta,
    options: &ReceiveOptions,
    resume: bool,
) -> Result<(), Error> {
    if target.exists() && !options.force {
        if resume && same_blob(target, hash, size)? {
            return finish_file(target, meta.mtime_ms, meta.mode, options.no_times);
        }
        return Err(storage::clash(target));
    }
    let incoming = storage::incoming_path(target);
    if size == 0 {
        fs::write(&incoming, []).map_err(|e| storage::storage("create", target, e))?;
    } else {
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
    finish_file(target, meta.mtime_ms, meta.mode, options.no_times)
}

fn finish_file(target: &Path, mtime_ms: u64, mode: u32, no_times: bool) -> Result<(), Error> {
    if !no_times {
        set_time(target, mtime_ms)?;
    }
    set_mode(target, mode)
}

#[cfg(unix)]
fn write_link(
    entry: &crate::manifest::Entry,
    options: &ReceiveOptions,
    resume: bool,
) -> Result<(), Error> {
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
    if let Ok(meta) = fs::symlink_metadata(&link) {
        if options.force {
            fs::remove_file(&link).map_err(|e| storage::policy_path("replace", &link, e))?;
        } else if resume
            && meta.file_type().is_symlink()
            && fs::read_link(&link).ok().as_deref() == Some(Path::new(target))
        {
            return Ok(());
        } else {
            return Err(storage::clash(&link));
        }
    }
    std::os::unix::fs::symlink(target, &link)
        .map_err(|e| storage::policy_path("create a symlink at", &link, e))
}

#[cfg(windows)]
fn write_link(
    entry: &crate::manifest::Entry,
    options: &ReceiveOptions,
    resume: bool,
) -> Result<(), Error> {
    let Some(target) = entry.link_target.as_ref().map(|target| target.as_str()) else {
        return Err(Error::Bug("a symlink entry has no target".to_string()));
    };
    let link = options.out.join(entry.path.as_str());
    if let Ok(meta) = fs::symlink_metadata(&link) {
        if options.force {
            fs::remove_file(&link).map_err(|e| storage::policy_path("replace", &link, e))?;
        } else if resume
            && meta.file_type().is_symlink()
            && fs::read_link(&link).ok().as_deref() == Some(Path::new(target))
        {
            return Ok(());
        } else {
            return Err(storage::clash(&link));
        }
    }
    std::os::windows::fs::symlink_file(target, &link).map_err(|e| {
        Error::Policy(format!(
            "cannot create a symlink at {}: {e}; Windows needs developer mode or an elevated \
             process for links",
            link.display()
        ))
    })
}

#[cfg(not(any(unix, windows)))]
fn write_link(
    _entry: &crate::manifest::Entry,
    _options: &ReceiveOptions,
    _resume: bool,
) -> Result<(), Error> {
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

#[cfg(test)]
mod tests {
    use iroh_blobs::Hash;

    #[test]
    fn an_existing_file_is_recognised_by_its_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.bin");
        std::fs::write(&path, b"the same bytes").unwrap();
        let hash = Hash::from(*blake3::hash(b"the same bytes").as_bytes());
        assert!(super::same_blob(&path, hash, 14).unwrap());
        let other = Hash::from(*blake3::hash(b"other").as_bytes());
        assert!(!super::same_blob(&path, other, 14).unwrap());
        assert!(!super::same_blob(&path, hash, 13).unwrap());
    }
}
