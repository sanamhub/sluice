//! The sender's side: a manifest from a folder, and the list of what a receiver would refuse.
//!
//! A receiver rejects a manifest that breaks a rule, so a sender that built one anyway would send
//! a folder that cannot arrive. [`Manifest::build`] finds every name that would be rejected and
//! lists them all, so one run tells the user everything to fix. It returns that list as data and
//! not as an error message: error text can reach a log, and file names stay out of logs.

mod assemble;
mod collide;
mod disk;
mod finding;
mod hash;
mod meta;
mod walk;

use std::path::Path;

pub use disk::DiskPaths;
pub use finding::{Finding, Reason};

use super::{Limits, Manifest};
use crate::Error;

/// A manifest, where its files are on disk, and the entries that were left out of it.
///
/// # Examples
///
/// ```
/// use sluice_core::manifest::{Build, Manifest};
///
/// let folder = tempfile::tempdir()?;
/// std::fs::write(folder.path().join("a.txt"), "hello")?;
///
/// let Build::Ready(built) = Manifest::build(folder.path())? else {
///     panic!("nothing in the folder is refused");
/// };
/// assert_eq!(built.manifest.entries().len(), 1);
/// assert!(built.skipped.is_empty());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug)]
pub struct Built {
    /// The manifest of everything that was sent.
    pub manifest: Manifest,
    /// The file on disk behind each `File` entry, by the entry's index.
    pub disk: DiskPaths,
    /// Symlinks with a target another machine cannot use, and special files. Left out, and
    /// listed so the user knows.
    pub skipped: Vec<Finding>,
}

/// Why nothing was built: names that a receiver would reject.
///
/// # Examples
///
/// ```
/// use sluice_core::manifest::{Build, Manifest, Reason};
///
/// let folder = tempfile::tempdir()?;
/// // U+202E reverses what follows it, so this shows as `reportexe.pdf` and ends in `.exe`.
/// std::fs::write(folder.path().join("report\u{202e}fdp.exe"), "a")?;
///
/// let Build::Refused(refusal) = Manifest::build(folder.path())? else {
///     panic!("the name breaks a rule");
/// };
/// assert_eq!(refusal.problems.len(), 1);
/// assert_eq!(refusal.problems[0].reason, Reason::PathRule(6));
/// // The error says how many, and names none of them.
/// assert!(!refusal.error().to_string().contains("report"));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug)]
pub struct Refusal {
    /// Every name a receiver would reject, sorted by path.
    pub problems: Vec<Finding>,
    /// What would have been skipped, so the user sees all of it in one run.
    pub skipped: Vec<Finding>,
}

impl Refusal {
    /// An [`Error::Policy`] that holds counts and no name, for a caller that has nowhere to put
    /// the list. The list is the thing to show people.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::manifest::{Finding, Reason, Refusal};
    ///
    /// let refusal = Refusal {
    ///     problems: vec![Finding { path: "secret/con".into(), reason: Reason::PathRule(8) }],
    ///     skipped: vec![],
    /// };
    /// let message = refusal.error().to_string();
    /// assert!(message.contains("1 name"));
    /// assert!(!message.contains("secret"));
    /// ```
    pub fn error(&self) -> Error {
        Error::Policy(format!(
            "the folder holds {} names a receiver would reject, so nothing was built",
            self.problems.len()
        ))
    }
}

/// What [`Manifest::build`] made of a folder.
///
/// It is one type with two cases, and not an error for the second, because the list is the
/// answer: the CLI prints it and `--json` can return it. An `Err` is for what is wrong with the
/// build itself, such as a folder that cannot be opened.
///
/// # Examples
///
/// ```
/// use sluice_core::manifest::{Build, Manifest};
///
/// let folder = tempfile::tempdir()?;
/// match Manifest::build(folder.path())? {
///     Build::Ready(built) => assert!(built.manifest.entries().is_empty()),
///     Build::Refused(refusal) => panic!("{} problems", refusal.problems.len()),
/// }
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug)]
pub enum Build {
    /// A manifest, and what was left out of it.
    Ready(Built),
    /// A name a receiver would reject. Nothing was hashed and nothing is sent.
    Refused(Refusal),
}

impl Manifest {
    /// Builds the manifest of a folder, with a `Dir` entry for every folder under it. A file of 1
    /// byte to under [`PACK_THRESHOLD`](super::pack::PACK_THRESHOLD) goes into a pack, as
    /// `SPEC.md` section 9 says, and any other file is a blob with its BLAKE3 root. The packs are
    /// in [`Manifest::packs`]. The folder itself is not an entry. It is read in place and not
    /// changed, and no pack is written to disk.
    ///
    /// The walk never follows a symlink, except `root` itself when it is one. Hidden files are
    /// included. Hard links are separate files. A name a receiver would reject refuses the whole
    /// build and every such name is listed, before any file is hashed. A symlink whose target a
    /// receiver would reject, a socket, a FIFO and a device are left out and listed.
    ///
    /// The size and modification time of each file are read before it is hashed and after. If
    /// either changed, or the bytes read are not the size, the build fails with
    /// [`Error::Storage`], because the hash would match no version of the file.
    ///
    /// A folder whose manifest a receiver on the default [`Limits`] would reject fails the build,
    /// because it could never arrive. The entry count is checked before any file is hashed.
    ///
    /// # Errors
    ///
    /// [`Error::Storage`] when `root` cannot be read or a file changed while it was read, and
    /// [`Error::Policy`] when `root` is not a folder or the manifest would be over a limit. No
    /// message holds a name.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::manifest::{Build, Kind, Manifest};
    ///
    /// let folder = tempfile::tempdir()?;
    /// std::fs::create_dir(folder.path().join("docs"))?;
    /// std::fs::write(folder.path().join("docs").join("a.txt"), "hello")?;
    ///
    /// let Build::Ready(built) = Manifest::build(folder.path())? else {
    ///     panic!("nothing is refused");
    /// };
    /// let entries = built.manifest.entries();
    /// let kinds: Vec<_> = entries.iter().map(|e| (e.path.as_str(), e.kind)).collect();
    /// assert_eq!(kinds, [("docs", Kind::Dir), ("docs/a.txt", Kind::File)]);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn build(root: &Path) -> Result<Build, Error> {
        Self::build_with_limits(root, Limits::default())
    }

    /// [`Manifest::build`], refusing a folder whose manifest would be over `limits` instead of
    /// the defaults. A sender has no reason to raise them, because a receiver on the defaults
    /// would reject what it sent. A test lowers them to reach them with a small folder.
    ///
    /// # Errors
    ///
    /// As for [`Manifest::build`], with `limits` in place of the defaults.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::{Error, manifest::{Limits, Manifest}};
    ///
    /// let folder = tempfile::tempdir()?;
    /// for name in ["a", "b", "c"] {
    ///     std::fs::write(folder.path().join(name), "x")?;
    /// }
    /// let limits = Limits { max_entries: 2, ..Limits::default() };
    /// let err = Manifest::build_with_limits(folder.path(), limits).unwrap_err();
    /// assert!(matches!(err, Error::Policy(_)));
    /// assert!(err.to_string().contains("3 entries"));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn build_with_limits(root: &Path, limits: Limits) -> Result<Build, Error> {
        build(root, limits, |_| {})
    }
}

/// [`Manifest::build_with_limits`] with a hook that runs for each file between its last byte and
/// the second look at its size and time, so a test can change a file there, which a real run
/// cannot do on purpose.
fn build(root: &Path, limits: Limits, between: impl Fn(&Path)) -> Result<Build, Error> {
    let walked = walk::walk(root)?;
    // Before the names are compared and before any file is hashed: renaming files does not
    // bring the count down, so a list of names to fix would not help.
    check_entries(walked.items.len(), limits)?;
    let mut problems = walked.problems;
    problems.extend(collide::find(&walked.items));
    if !problems.is_empty() {
        return Ok(refuse(problems, walked.skipped));
    }
    let mut items = walked.items;
    // Packing follows manifest order, so the files are read in that order.
    items.sort_by(|a, b| a.path.as_str().cmp(b.path.as_str()));
    let assembled = assemble::assemble(items, &between).map_err(|assemble::Changed| {
        Error::Storage(
            "a file changed while it was read, so its hash would match no version of it"
                .to_string(),
        )
    })?;
    if !assembled.problems.is_empty() {
        return Ok(refuse(assembled.problems, walked.skipped));
    }
    // The walk and `collide` found everything `new` looks for, and the planner follows rule 16,
    // so this is a check of the builder.
    let manifest = Manifest::new(assembled.entries, assembled.packs).map_err(|e| {
        Error::Bug(format!(
            "the builder made a manifest that fails a rule: {e}"
        ))
    })?;
    // The length depends on the sizes and times the hash read, so it is known only now. At the
    // default limits a folder under the entry limit is rarely over the byte limit.
    check_bytes(manifest.encode().len(), limits)?;
    let mut skipped = walked.skipped;
    skipped.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(Build::Ready(Built {
        manifest,
        disk: DiskPaths::new(assembled.disk),
        skipped,
    }))
}

/// Decoding rule 2 at the sender. The message gives numbers and no name.
fn check_entries(entries: usize, limits: Limits) -> Result<(), Error> {
    if u64::try_from(entries).unwrap_or(u64::MAX) > limits.max_entries {
        return Err(Error::Policy(format!(
            "the folder has {entries} entries and a receiver accepts at most {}, so nothing was \
             built",
            limits.max_entries
        )));
    }
    Ok(())
}

/// Decoding rule 1 at the sender.
fn check_bytes(bytes: usize, limits: Limits) -> Result<(), Error> {
    if u64::try_from(bytes).unwrap_or(u64::MAX) > limits.max_bytes {
        return Err(Error::Policy(format!(
            "the manifest of the folder would be {bytes} bytes and a receiver accepts at most {}, \
             so it is not sent",
            limits.max_bytes
        )));
    }
    Ok(())
}

fn refuse(mut problems: Vec<Finding>, mut skipped: Vec<Finding>) -> Build {
    problems.sort_by(|a, b| a.path.cmp(&b.path));
    skipped.sort_by(|a, b| a.path.cmp(&b.path));
    Build::Refused(Refusal { problems, skipped })
}

#[cfg(test)]
mod tests;
