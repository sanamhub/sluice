//! Every check a receive makes before the first content request: the ticket's size against the
//! manifest, the entries a selection takes, the top-level names, hidden names, every target path,
//! every clash, every ancestor that could be a symlink, and the free space for the bytes fetched.
//!
//! The entries a selection takes and the bytes it will fetch are here; the checks that look at the
//! output folder are in [`target`], and the bytes a fetch moves are in [`fetched`].

use std::collections::BTreeSet;

use crate::{
    Error,
    engine::Selection,
    manifest::{Kind, Manifest},
    ticket::Ticket,
};

use super::ReceiveOptions;

mod fetched;
mod target;

pub(in crate::engine) use fetched::{fetched_bytes, pack, packs_of, shared_bytes, size_overflow};
pub(super) use target::check_space;
pub use target::top_level;
pub(in crate::engine) use target::{check_one, check_promised};

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
    /// The bytes the content requests will move. A packed file is fetched inside its whole pack, so
    /// this can be more than `bytes`; it is what the free-space check and the progress total use.
    pub fetched: u64,
    /// The top-level names this receive made, which a folder that is already there belongs to. A
    /// folder under one of them was made by the run this one continues, so it gets its mode and
    /// time like any other.
    pub ours: std::collections::BTreeSet<String>,
    /// The entries this receive writes, by position in the manifest, in manifest order.
    pub chosen: Vec<usize>,
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
    selection: &Selection,
) -> Result<(Plan, BTreeSet<String>), Error> {
    let all = totals(manifest)?;
    // The ticket promises the size of the whole manifest, even when this run takes only part of it.
    check_promised(ticket, all.bytes)?;
    let chosen = choose(manifest, selection)?;
    let taken = totals_of(manifest, &chosen)?;
    // A selection fetches each pack that holds a taken file whole, so the room it needs is the
    // fetched bytes and not the bytes that land on disk.
    let fetched = fetched_bytes(manifest, &chosen)?;
    check_space(options, fetched)?;
    let created = target::check_top(manifest, options, ours, &chosen)?;
    let made = created.union(ours).cloned().collect();
    for &at in &chosen {
        let entry = &manifest.entries()[at];
        let target = entry.path.under(&options.out);
        target::check_ancestors(&options.out, &target)?;
        target::check_target(&target, entry.kind, options.force || resume)?;
    }
    Ok((
        Plan {
            files: taken.files,
            folders: taken.folders,
            links: taken.links,
            bytes: taken.bytes,
            fetched,
            ours: made,
            chosen,
        },
        created,
    ))
}

/// The entries a selection takes, by position in the manifest, with the folder entries above them: a
/// folder is made when something under it is taken, and one a pattern names itself is taken too. No
/// patterns take everything. A selection on a one-file ticket is refused, because there is nothing
/// to choose between and the ticket's one name is the file.
pub(in crate::engine) fn choose(
    manifest: &Manifest,
    selection: &Selection,
) -> Result<Vec<usize>, Error> {
    let entries = manifest.entries();
    if selection.is_empty() {
        return Ok((0..entries.len()).collect());
    }
    if matches!(entries, [entry] if entry.kind == Kind::File) {
        return Err(Error::Policy(
            "--include takes some of a folder ticket's entries, and this ticket is one file"
                .to_string(),
        ));
    }
    let by_path: std::collections::BTreeMap<&str, usize> = entries
        .iter()
        .enumerate()
        .map(|(at, entry)| (entry.path.as_str(), at))
        .collect();
    let mut chosen: BTreeSet<usize> = entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| selection.selects(entry.path.as_str()))
        .map(|(at, _)| at)
        .collect();
    if chosen.is_empty() {
        let patterns = selection.patterns().join(", ");
        return Err(Error::Policy(format!(
            "the pattern{} {patterns} match{} nothing in this ticket",
            if selection.patterns().len() == 1 {
                ""
            } else {
                "s"
            },
            if selection.patterns().len() == 1 {
                "es"
            } else {
                ""
            },
        )));
    }
    for at in chosen.clone() {
        for ancestor in ancestors(entries[at].path.as_str()) {
            if let Some(&found) = by_path.get(ancestor) {
                chosen.insert(found);
            }
        }
    }
    Ok(chosen.into_iter().collect())
}

/// The folder paths above `path`, innermost first: `a/b/c.txt` gives `a/b` and `a`.
fn ancestors(path: &str) -> impl Iterator<Item = &str> {
    path.match_indices('/').map(move |(at, _)| &path[..at])
}

/// How many of each kind there are and how many bytes the files add up to. A manifest comes from a
/// stranger, so the sum is checked: in a release build a plain `+` would wrap, and a wrapped total
/// could be made to match a ticket's size and pass the free-space check for a tree far larger than
/// it.
pub(in crate::engine) fn totals(manifest: &Manifest) -> Result<Totals, Error> {
    let all: Vec<usize> = (0..manifest.entries().len()).collect();
    totals_of(manifest, &all)
}

/// [`totals`] over the entries a receive takes, by position in the manifest.
fn totals_of(manifest: &Manifest, chosen: &[usize]) -> Result<Totals, Error> {
    let mut totals = Totals::default();
    for &at in chosen {
        let entry = &manifest.entries()[at];
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

#[cfg(test)]
mod tests {
    use super::totals;

    /// A release build has no overflow checks, so a hostile manifest whose sizes add past
    /// `u64::MAX` must be refused before any sum wraps and matches a ticket's size.
    #[test]
    fn file_sizes_that_add_past_u64_max_are_refused() {
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
