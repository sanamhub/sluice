//! Every check a receive makes before the first content request: the ticket's size against the
//! manifest, the top-level names, hidden names, every target path, every clash, every ancestor that
//! could be a symlink, and the free space for the whole tree.

use std::{collections::BTreeSet, fs, path::Path};

use super::ReceiveOptions;
use crate::{
    Error,
    manifest::{Entry, Kind, Manifest},
    ticket::{Body, Ticket},
};

/// What the checked manifest will write.
#[derive(Debug)]
pub(super) struct Plan {
    /// How many files.
    pub files: u64,
    /// How many folders.
    pub folders: u64,
    /// How many symlinks.
    pub links: u64,
    /// Their total size in bytes.
    pub bytes: u64,
    /// The top-level names this receive made, which a folder that is already there belongs to. A
    /// folder under one of them was made by the run this one continues, so it gets its mode and
    /// time like any other.
    pub ours: std::collections::BTreeSet<String>,
}

/// What the manifest will write, counted by kind.
#[derive(Debug, Default)]
pub(in crate::engine) struct Totals {
    pub files: u64,
    pub folders: u64,
    pub links: u64,
    pub bytes: u64,
}

/// Checks the whole manifest. Nothing here writes, so a refusal leaves the output untouched. The
/// second value is the top-level names this receive will create, which are the only ones a resume
/// may write into: an entry that was already in the output belongs to the user.
pub(super) fn check(
    manifest: &Manifest,
    ticket: &Ticket,
    options: &ReceiveOptions,
    resume: bool,
    ours: &BTreeSet<String>,
) -> Result<(Plan, BTreeSet<String>), Error> {
    let totals = totals(manifest)?;
    check_promised(ticket, totals.bytes)?;
    check_space(options, totals.bytes)?;
    let created = check_top(manifest, options, ours)?;
    let made = created.union(ours).cloned().collect();
    for entry in manifest.entries() {
        let target = entry.path.under(&options.out);
        check_ancestors(&options.out, &target)?;
        check_target(&target, entry.kind, options.force || resume)?;
    }
    Ok((
        Plan {
            files: totals.files,
            folders: totals.folders,
            links: totals.links,
            bytes: totals.bytes,
            ours: made,
        },
        created,
    ))
}

/// How many of each kind there are and how many bytes the files add up to. A manifest comes from a
/// stranger, so the sum is checked: in a release build a plain `+` would wrap, and a wrapped total
/// could be made to match a ticket's size and pass the free-space check for a tree far larger than
/// it.
pub(in crate::engine) fn totals(manifest: &Manifest) -> Result<Totals, Error> {
    let mut totals = Totals::default();
    for entry in manifest.entries() {
        match entry.kind {
            Kind::File => {
                // The decoder's entry limit is far below `u64::MAX`, so this cannot pass it.
                totals.files += 1;
                totals.bytes = totals
                    .bytes
                    .checked_add(entry.size)
                    .ok_or_else(size_overflow)?;
            }
            Kind::Dir => totals.folders += 1,
            Kind::Symlink => totals.links += 1,
        }
    }
    Ok(totals)
}

pub(super) fn size_overflow() -> Error {
    Error::Policy("the manifest's file sizes add up past u64::MAX".to_string())
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

pub(super) fn check_space(options: &ReceiveOptions, bytes: u64) -> Result<(), Error> {
    let available = match options.free_bytes {
        Some(bytes) => bytes,
        None => super::storage::available_bytes(&options.out)?,
    };
    // A volume that cannot link a finished file out of the store has to copy it while the store
    // still holds the blob, so the whole content is there twice.
    let needed = super::storage::needed_bytes(bytes, super::storage::supports_links(&options.out));
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
/// against their content by the writer. Returns the top-level names this receive creates.
fn check_top(
    manifest: &Manifest,
    options: &ReceiveOptions,
    ours: &BTreeSet<String>,
) -> Result<BTreeSet<String>, Error> {
    let mut created = BTreeSet::new();
    for entry in manifest.entries() {
        let name = entry.path.as_str();
        if name.contains('/') {
            continue;
        }
        let target = entry.path.under(&options.out);
        let exists = match fs::symlink_metadata(&target) {
            Ok(_) => true,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
            Err(e) => return Err(super::storage::policy_path("check", &target, e)),
        };
        match top_level(
            name,
            exists,
            options.allow_hidden,
            options.force,
            ours.contains(name),
        ) {
            Ok(()) => {
                if !exists {
                    created.insert(name.to_string());
                }
            }
            Err("hidden") => {
                return Err(Error::Policy(format!(
                    "the transfer would write {name}, whose name starts with a dot; pass \
                     --allow-hidden to accept a top-level hidden name"
                )));
            }
            Err(_) => return Err(super::storage::clash(&target)),
        }
    }
    Ok(created)
}

/// The checks a range download makes: a transfer of exactly one file, its top-level name and the
/// target itself. The whole-tree size and free-space checks do not apply, because most of the file
/// is never fetched or written.
pub(in crate::engine) fn check_one<'a>(
    manifest: &'a Manifest,
    options: &ReceiveOptions,
    resume: bool,
) -> Result<&'a Entry, Error> {
    let [entry] = manifest.entries() else {
        return Err(Error::Policy(
            "a range works on a one-file transfer, and this ticket is a folder".to_string(),
        ));
    };
    if entry.kind != Kind::File {
        return Err(Error::Policy(
            "a range works on a one-file transfer, and this ticket is a folder".to_string(),
        ));
    }
    let name = entry.path.as_str();
    if name.starts_with('.') && !options.allow_hidden {
        return Err(Error::Policy(format!(
            "the transfer would write {name}, whose name starts with a dot; pass --allow-hidden \
             to accept a top-level hidden name"
        )));
    }
    let target = entry.path.under(&options.out);
    if !options.force && !resume && fs::symlink_metadata(&target).is_ok() {
        return Err(super::storage::clash(&target));
    }
    check_ancestors(&options.out, &target)?;
    check_target(&target, entry.kind, options.force || resume)?;
    Ok(entry)
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

    /// A release build has no overflow checks, so a hostile manifest whose sizes add past
    /// `u64::MAX` must be refused before any sum wraps and matches a ticket's size.
    #[test]
    fn file_sizes_that_add_past_u64_max_are_refused() {
        use super::totals;
        use crate::manifest::{Entry, Kind, Loc, Manifest, path::ManifestPath};

        let entry = |name: &str, root: [u8; 32], size: u64| Entry {
            path: ManifestPath::parse(name).unwrap(),
            kind: Kind::File,
            size,
            mtime_ms: 0,
            mode: 0,
            loc: Loc::Blob { root },
            link_target: None,
        };
        let manifest = Manifest::new(
            vec![entry("a", [1; 32], u64::MAX), entry("b", [2; 32], 1)],
            vec![],
        )
        .unwrap();
        let err = totals(&manifest).unwrap_err();
        assert!(matches!(err, crate::Error::Policy(_)), "{err}");
        assert!(err.to_string().contains("u64::MAX"), "{err}");
    }
}
