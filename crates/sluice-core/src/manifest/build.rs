//! The sender's side: a manifest from a folder, and the list of what a receiver would refuse.
//!
//! A receiver rejects a manifest that breaks a rule, so a sender that built one anyway would send
//! a folder that cannot arrive. [`Manifest::build`] finds every name that would be rejected and
//! lists them all, so one run tells the user everything to fix. It returns that list as data and
//! not as an error message: error text can reach a log, and file names stay out of logs.

mod collide;
mod hash;
mod meta;
mod walk;

use std::fmt;
use std::io;
use std::path::Path;

use super::Manifest;
use super::entry::{Entry, Kind, Loc};
use super::path::ManifestPath;
use crate::Error;
use hash::{HashFault, Hashed};
use walk::{Item, What};

/// Why a name was listed. The wording is for people; a program reads the variant.
///
/// # Examples
///
/// ```
/// use sluice_core::manifest::Reason;
///
/// assert_eq!(Reason::NotUtf8.to_string(), "the name is not valid UTF-8");
/// assert_eq!(
///     Reason::PathRule(7).to_string(),
///     "a receiver rejects the name (path rule 7)"
/// );
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    /// The name breaks path rule 1 to 8, the number given. Rule 2 is never given: the sender
    /// writes the NFC form of every name.
    PathRule(u8),
    /// The name on disk is not valid UTF-8, so it has no path in a manifest.
    NotUtf8,
    /// Another name on disk is the same path after NFC. Which one a receiver would write is
    /// not defined, so neither is sent.
    SameAfterNfc,
    /// The path is the same as another after full case folding and NFC.
    CaseCollision,
    /// A file or folder could not be opened or read.
    Unreadable(io::ErrorKind),
    /// A symlink whose target breaks link target rule 1 to 5, the number given. Skipped.
    LinkTarget(u8),
    /// A symlink whose target is not valid UTF-8. Skipped.
    LinkTargetNotUtf8,
    /// A socket, FIFO, device or other entry that is neither file, folder nor symlink. Skipped.
    NotAFileOrFolder,
}

impl fmt::Display for Reason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PathRule(rule) => write!(f, "a receiver rejects the name (path rule {rule})"),
            Self::NotUtf8 => f.write_str("the name is not valid UTF-8"),
            Self::SameAfterNfc => f.write_str("another name on disk is the same path after NFC"),
            Self::CaseCollision => f.write_str("the same path as another after case folding"),
            Self::Unreadable(kind) => write!(f, "cannot be read ({kind})"),
            Self::LinkTarget(rule) => {
                write!(f, "the link target breaks link target rule {rule}")
            }
            Self::LinkTargetNotUtf8 => f.write_str("the link target is not valid UTF-8"),
            Self::NotAFileOrFolder => f.write_str("is not a file, a folder or a symlink"),
        }
    }
}

/// One name the build listed, and why. `path` is the path as the sender sees it, relative to the
/// folder that was sent, with `/` between components and the bytes that are not UTF-8 replaced
/// by U+FFFD. It is not escaped: a name can hold a newline or a terminal escape, and whoever
/// prints it has to deal with that.
///
/// # Examples
///
/// ```
/// use sluice_core::manifest::{Finding, Reason};
///
/// let finding = Finding { path: "a/con".to_string(), reason: Reason::PathRule(8) };
/// assert_eq!(finding.reason, Reason::PathRule(8));
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    /// The path as the sender sees it.
    pub path: String,
    /// Why it was listed.
    pub reason: Reason,
}

/// A manifest, and the entries that were left out of it.
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
    /// Builds the manifest of a folder, with a `Dir` entry for every folder under it and a
    /// BLAKE3 root for every file. The folder itself is not an entry. It is read in place and
    /// not changed.
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
    /// # Errors
    ///
    /// [`Error::Storage`] when `root` cannot be read or a file changed while it was read, and
    /// [`Error::Policy`] when `root` is not a folder. Neither message holds a name.
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
        build(root, hash::hash_file)
    }
}

/// [`Manifest::build`] with the hashing passed in, so a test can change a file between the two
/// reads of its size and time, which a real run cannot do on purpose.
fn build(
    root: &Path,
    hash_file: impl Fn(&Path) -> Result<Hashed, HashFault>,
) -> Result<Build, Error> {
    let walked = walk::walk(root)?;
    let mut problems = walked.problems;
    problems.extend(collide::find(&walked.items));
    if !problems.is_empty() {
        return Ok(refuse(problems, walked.skipped));
    }
    let mut entries = Vec::with_capacity(walked.items.len());
    for item in walked.items {
        match entry_of(item, &hash_file) {
            Made::Entry(entry) => entries.push(entry),
            Made::Listed(finding) => problems.push(finding),
            Made::Changed => {
                return Err(Error::Storage(
                    "a file changed while it was read, so its hash would match no version of it"
                        .to_string(),
                ));
            }
        }
    }
    if !problems.is_empty() {
        return Ok(refuse(problems, walked.skipped));
    }
    entries.sort_by(|a, b| a.path.as_str().cmp(b.path.as_str()));
    // The walk and `collide` found everything `new` looks for, so this is a check of the builder.
    let manifest = Manifest::new(entries, Vec::new()).map_err(|e| {
        Error::Bug(format!(
            "the builder made a manifest that fails a rule: {e}"
        ))
    })?;
    let mut skipped = walked.skipped;
    skipped.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(Build::Ready(Built { manifest, skipped }))
}

fn refuse(mut problems: Vec<Finding>, mut skipped: Vec<Finding>) -> Build {
    problems.sort_by(|a, b| a.path.cmp(&b.path));
    skipped.sort_by(|a, b| a.path.cmp(&b.path));
    Build::Refused(Refusal { problems, skipped })
}

/// What one item came to.
enum Made {
    Entry(Entry),
    /// A file that cannot be read. Listed, and the build goes on so every such file is found.
    Listed(Finding),
    /// A file that changed while it was read. The build stops.
    Changed,
}

/// The entry for one item. A file is hashed here.
fn entry_of(item: Item, hash_file: &impl Fn(&Path) -> Result<Hashed, HashFault>) -> Made {
    let Item { path, shown, what } = item;
    match what {
        What::Dir { mtime_ms, mode } => Made::Entry(folder_entry(path, mtime_ms, mode)),
        What::Symlink { target, mtime_ms } => Made::Entry(Entry {
            path,
            kind: Kind::Symlink,
            size: 0,
            mtime_ms,
            mode: 0,
            loc: Loc::None,
            link_target: Some(target),
        }),
        What::File { disk } => match hash_file(&disk) {
            Ok(hashed) => Made::Entry(file_entry(path, hashed)),
            Err(HashFault::Unreadable(kind)) => Made::Listed(Finding {
                path: shown,
                reason: Reason::Unreadable(kind),
            }),
            Err(HashFault::Changed) => Made::Changed,
        },
    }
}

fn folder_entry(path: ManifestPath, mtime_ms: u64, mode: u32) -> Entry {
    Entry {
        path,
        kind: Kind::Dir,
        size: 0,
        mtime_ms,
        mode,
        loc: Loc::None,
        link_target: None,
    }
}

fn file_entry(path: ManifestPath, hashed: Hashed) -> Entry {
    Entry {
        path,
        kind: Kind::File,
        size: hashed.size,
        mtime_ms: hashed.mtime_ms,
        mode: hashed.mode,
        loc: Loc::Blob { root: hashed.root },
        link_target: None,
    }
}

#[cfg(test)]
mod tests;
