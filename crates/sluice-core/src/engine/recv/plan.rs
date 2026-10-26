//! Every check a folder receive makes before the first content request: the ticket's size against
//! the manifest, every target path, every clash, every ancestor that could be a symlink, and the
//! free space for the whole tree.

use std::{fs, path::Path};

use super::ReceiveOptions;
use crate::{
    Error,
    manifest::{Kind, Manifest},
    ticket::{Body, Ticket},
};

/// What the checked manifest will write.
#[derive(Clone, Copy, Debug)]
pub(super) struct Plan {
    /// How many files.
    pub files: u64,
    /// Their total size in bytes.
    pub bytes: u64,
}

/// Checks the whole manifest. Nothing here writes, so a refusal leaves the output untouched.
pub(super) fn check(
    manifest: &Manifest,
    ticket: &Ticket,
    options: &ReceiveOptions,
) -> Result<Plan, Error> {
    let files = manifest
        .entries()
        .iter()
        .filter(|entry| entry.kind == Kind::File)
        .count() as u64;
    let bytes = manifest
        .entries()
        .iter()
        .filter(|entry| entry.kind == Kind::File)
        .map(|entry| entry.size)
        .sum();
    check_promised(ticket, bytes)?;
    check_space(options, bytes)?;
    for entry in manifest.entries() {
        let target = options.out.join(entry.path.as_str());
        check_ancestors(&options.out, &target)?;
        check_target(&target, entry.kind, options.force)?;
    }
    Ok(Plan { files, bytes })
}

fn check_promised(ticket: &Ticket, bytes: u64) -> Result<(), Error> {
    let promised = match ticket.body() {
        Body::Static { size, .. } => *size,
        Body::Live { .. } => 0,
    };
    if promised != bytes {
        return Err(Error::Policy(format!(
            "the ticket promises {promised} bytes and the manifest says {bytes}"
        )));
    }
    Ok(())
}

fn check_space(options: &ReceiveOptions, bytes: u64) -> Result<(), Error> {
    let available = match options.free_bytes {
        Some(bytes) => bytes,
        None => super::storage::available_bytes(&options.out)?,
    };
    let needed = super::storage::needed_bytes(bytes);
    if available < needed {
        return Err(Error::Storage(format!(
            "{} has {available} bytes free and this transfer needs about {needed}",
            options.out.display()
        )));
    }
    Ok(())
}

/// Refuses a path whose existing parent is a symlink or not a folder. Without this, a manifest path
/// that passes every text rule could still write through a link someone placed in the output.
fn check_ancestors(out: &Path, target: &Path) -> Result<(), Error> {
    let relative = target
        .strip_prefix(out)
        .map_err(|_| Error::Bug("a manifest path left the output folder".to_string()))?;
    let components: Vec<_> = relative.components().collect();
    let mut at = out.to_path_buf();
    for component in components.iter().take(components.len().saturating_sub(1)) {
        at.push(component);
        match fs::symlink_metadata(&at) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(Error::Policy(format!(
                    "{} is a symlink, so a file would be written through it",
                    at.display()
                )));
            }
            Ok(meta) if !meta.is_dir() => {
                return Err(Error::Policy(format!(
                    "{} is not a folder, so nothing can be written under it",
                    at.display()
                )));
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => break,
            Err(e) => return Err(super::storage::policy_path("check", &at, e)),
        }
    }
    Ok(())
}

fn check_target(target: &Path, kind: Kind, force: bool) -> Result<(), Error> {
    let meta = match fs::symlink_metadata(target) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(super::storage::policy_path("check", target, e)),
    };
    match kind {
        // A folder entry may name a folder that is already there; its own entries are checked
        // separately. Replacing a folder with a file is never allowed, with or without --force.
        Kind::Dir if meta.is_dir() => Ok(()),
        Kind::Dir => Err(Error::Policy(format!(
            "{} is not a folder, so the folder entry cannot be created",
            target.display()
        ))),
        Kind::File | Kind::Symlink if meta.is_dir() => Err(Error::Policy(format!(
            "{} is a folder, so --force cannot replace it with a file",
            target.display()
        ))),
        Kind::File | Kind::Symlink if force => Ok(()),
        _ => Err(super::storage::clash(target)),
    }
}
