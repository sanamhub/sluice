//! Every check a receive makes before the first content request: the ticket's size against the
//! manifest, the top-level names, hidden names, every target path, every clash, every ancestor that
//! could be a symlink, and the free space for the whole tree.

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
    resume: bool,
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
    check_top(manifest, options, resume)?;
    for entry in manifest.entries() {
        let target = options.out.join(entry.path.as_str());
        check_ancestors(&options.out, &target)?;
        check_target(&target, entry.kind, options.force || resume)?;
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

/// The top-level rules: a name that starts with a dot needs `--allow-hidden`, and a name already in
/// the output folder refuses the whole transfer before any content is fetched. A resumed transfer
/// made the folder it is writing into, so it may write there again; its files are still checked
/// against their content by the writer.
fn check_top(manifest: &Manifest, options: &ReceiveOptions, resume: bool) -> Result<(), Error> {
    for entry in manifest.entries() {
        let name = entry.path.as_str();
        if name.contains('/') {
            continue;
        }
        let target = options.out.join(name);
        let exists = match fs::symlink_metadata(&target) {
            Ok(_) => true,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
            Err(e) => return Err(super::storage::policy_path("check", &target, e)),
        };
        match top_level(name, exists, options.allow_hidden, options.force, resume) {
            Ok(()) => {}
            Err("hidden") => {
                return Err(Error::Policy(format!(
                    "the transfer would write {name}, whose name starts with a dot; pass \
                     --allow-hidden to accept a top-level hidden name"
                )));
            }
            Err(_) => return Err(super::storage::clash(&target)),
        }
    }
    Ok(())
}

/// The top-level rules as a decision that takes no file system, so the vectors can check every
/// combination. `Err("hidden")` is a hidden name the user did not allow and `Err("clash")` a name
/// already in the output. A resumed receive made the folder it writes into, so it may write there
/// again; its files are still checked against their content by the writer.
pub fn top_level(
    name: &str,
    exists: bool,
    allow_hidden: bool,
    replace: bool,
    resume: bool,
) -> Result<(), &'static str> {
    if name.contains('/') {
        return Ok(());
    }
    if name.starts_with('.') && !allow_hidden {
        return Err("hidden");
    }
    if exists && !(replace || resume) {
        return Err("clash");
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

/// A target that already exists is a clash unless it is a folder entry meeting its own folder, or
/// the caller allowed a replacement. A folder is never replaced with a file, with or without
/// `--force`: the rename could not do it, and saying so before the transfer is clearer.
fn check_target(target: &Path, kind: Kind, replace: bool) -> Result<(), Error> {
    let meta = match fs::symlink_metadata(target) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(super::storage::policy_path("check", target, e)),
    };
    match kind {
        Kind::Dir if meta.is_dir() => Ok(()),
        Kind::Dir => Err(Error::Policy(format!(
            "{} is not a folder, so the folder entry cannot be created",
            target.display()
        ))),
        Kind::File | Kind::Symlink if meta.is_dir() => Err(Error::Policy(format!(
            "{} is a folder, so --force cannot replace it with a file",
            target.display()
        ))),
        Kind::File | Kind::Symlink if replace => Ok(()),
        _ => Err(super::storage::clash(target)),
    }
}

#[cfg(test)]
mod tests {
    use super::top_level;

    #[test]
    fn a_hidden_top_level_name_needs_the_flag() {
        assert_eq!(top_level(".ssh", false, false, false, false), Err("hidden"));
        assert!(top_level(".ssh", false, true, false, false).is_ok());
        // Hidden names below the top are ordinary entries.
        assert!(top_level(".ssh/config", false, false, false, false).is_ok());
    }

    #[test]
    fn an_existing_top_level_name_is_a_clash_before_anything_is_fetched() {
        assert_eq!(top_level("proj", true, false, false, false), Err("clash"));
        assert!(top_level("proj", true, false, true, false).is_ok());
        assert!(top_level("proj", true, false, false, true).is_ok());
        assert!(top_level("proj/a", true, false, false, false).is_ok());
    }
}
