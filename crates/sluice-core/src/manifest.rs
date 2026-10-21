//! Directory manifests: what a sender lists and what a receiver checks before it writes anything.
//!
//! A manifest is a list of entries, one for every file, folder and symlink under the folder that
//! was sent. A receiver gets it from a stranger and uses it to decide where to write, so
//! [`Manifest::decode`] applies every rule of `SPEC.md` section 8 before it returns anything, and a
//! [`Manifest`] cannot be built in a state that breaks one.

mod build;
mod entry;
pub mod link;
pub mod path;
mod rules;
#[cfg(test)]
mod tests;
mod wire;

use crate::Error;
use rules::{Fault, check_all, rule};

pub use build::{Build, Built, DiskPaths, Finding, Reason, Refusal};
pub use entry::{Entry, Kind, Loc, PackRef};

/// How much a receiver reads before it refuses a manifest.
///
/// Both limits are checked before the manifest is read into memory, so a hostile sender cannot
/// make a receiver run out of it by declaring a huge manifest. A caller can raise or lower them.
///
/// # Examples
///
/// ```
/// use sluice_core::manifest::Limits;
///
/// let limits = Limits::default();
/// assert_eq!(limits.max_bytes, 256 << 20);
/// assert_eq!(limits.max_entries, 1_000_000);
///
/// // A caller that knows the size of the manifest blob refuses it before fetching it.
/// assert!(limits.check_bytes(1 << 20).is_ok());
/// assert!(limits.check_bytes((256 << 20) + 1).is_err());
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// The most bytes a manifest may have. The default is 256 MiB.
    pub max_bytes: u64,
    /// The most entries a manifest may list. The default is 1,000,000.
    pub max_entries: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_bytes: 256 << 20,
            max_entries: 1_000_000,
        }
    }
}

impl Limits {
    /// Refuses a manifest of `bytes` bytes that is over `max_bytes`. The size of the manifest
    /// blob is known before it is fetched, so a receiver can ask this first and fetch nothing.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::{Error, manifest::Limits};
    ///
    /// let limits = Limits { max_bytes: 1000, max_entries: 10 };
    /// assert!(limits.check_bytes(1000).is_ok());
    /// let err = limits.check_bytes(1001).unwrap_err();
    /// assert!(matches!(err, Error::Policy(_)));
    /// assert!(err.to_string().starts_with("policy denied: manifest rule 1"));
    /// ```
    pub fn check_bytes(&self, bytes: u64) -> Result<(), Error> {
        self.check(bytes).map_err(Fault::into_error)
    }

    fn check(&self, bytes: u64) -> Result<(), Fault> {
        if bytes > self.max_bytes {
            return Err(Fault::new(rule::BYTES));
        }
        Ok(())
    }
}

/// The list of entries a sender builds from a folder and a receiver checks before it writes
/// anything, and the packs that small files are stored in.
///
/// Entries are sorted by path, byte by byte and strictly. Every folder has an entry that comes
/// before the entries inside it, and no two paths are the same after case folding.
///
/// # Examples
///
/// ```
/// use sluice_core::manifest::{Entry, Kind, Limits, Loc, Manifest, path::ManifestPath};
///
/// let entries = vec![
///     Entry {
///         path: ManifestPath::parse("docs")?,
///         kind: Kind::Dir,
///         size: 0,
///         mtime_ms: 1_800_000_000_000,
///         mode: 0o755,
///         loc: Loc::None,
///         link_target: None,
///     },
///     Entry {
///         path: ManifestPath::parse("docs/readme.txt")?,
///         kind: Kind::File,
///         size: 12,
///         mtime_ms: 1_800_000_000_000,
///         mode: 0o644,
///         loc: Loc::Blob { root: [9; 32] },
///         link_target: None,
///     },
/// ];
/// let manifest = Manifest::new(entries, vec![])?;
///
/// let bytes = manifest.encode();
/// assert_eq!(Manifest::decode(&bytes, Limits::default())?, manifest);
/// assert_eq!(manifest.root(), *blake3::hash(&bytes).as_bytes());
/// # Ok::<(), sluice_core::Error>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Manifest {
    entries: Vec<Entry>,
    packs: Vec<PackRef>,
}

impl Manifest {
    /// Builds a manifest from entries, which have to be what a decoder would accept: sorted by
    /// path with no repeat, each with a folder entry for its parent, no two the same after case
    /// folding, and each agreeing with itself and with `packs`.
    ///
    /// The error is [`Error::Policy`]. It names the rule and never a path.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::manifest::{Entry, Kind, Loc, Manifest, path::ManifestPath};
    ///
    /// let file = |path: &str| -> Result<Entry, sluice_core::Error> {
    ///     Ok(Entry {
    ///         path: ManifestPath::parse(path)?,
    ///         kind: Kind::File,
    ///         size: 0,
    ///         mtime_ms: 0,
    ///         mode: 0o644,
    ///         loc: Loc::Blob { root: [0; 32] },
    ///         link_target: None,
    ///     })
    /// };
    /// assert!(Manifest::new(vec![file("a")?, file("b")?], vec![]).is_ok());
    /// // `Readme.md` and `README.md` are one name on a case-insensitive disk.
    /// assert!(Manifest::new(vec![file("README.md")?, file("Readme.md")?], vec![]).is_err());
    /// // Out of order.
    /// assert!(Manifest::new(vec![file("b")?, file("a")?], vec![]).is_err());
    /// # Ok::<(), sluice_core::Error>(())
    /// ```
    pub fn new(entries: Vec<Entry>, packs: Vec<PackRef>) -> Result<Self, Error> {
        check_all(&entries, &packs).map_err(Fault::into_error)?;
        Ok(Self { entries, packs })
    }

    /// Reads a manifest from its bytes and applies every decoding rule of `SPEC.md` section 8.
    ///
    /// The byte limit is checked before any byte is read, and the entry limit before the first
    /// entry. The error is [`Error::Policy`]. It names the rule, and the path rule or link target
    /// rule inside it where there is one, and it never quotes a path.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::manifest::{Limits, Manifest};
    ///
    /// // The empty manifest: version 1, no entries, no packs.
    /// let empty = Manifest::decode(&[1, 0, 0], Limits::default())?;
    /// assert!(empty.entries().is_empty());
    ///
    /// // Version 2 is not read.
    /// let err = Manifest::decode(&[2, 0, 0], Limits::default()).unwrap_err();
    /// assert!(err.to_string().starts_with("policy denied: manifest rule 3"));
    /// # Ok::<(), sluice_core::Error>(())
    /// ```
    pub fn decode(bytes: &[u8], limits: Limits) -> Result<Self, Error> {
        let (entries, packs) = wire::read(bytes, limits).map_err(Fault::into_error)?;
        Ok(Self { entries, packs })
    }

    /// The bytes of the manifest, in the one form [`Manifest::decode`] accepts.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::manifest::Manifest;
    ///
    /// let empty = Manifest::new(vec![], vec![])?;
    /// assert_eq!(empty.encode(), [1, 0, 0]);
    /// # Ok::<(), sluice_core::Error>(())
    /// ```
    pub fn encode(&self) -> Vec<u8> {
        wire::write(&self.entries, &self.packs)
    }

    /// The root of the manifest: the BLAKE3 hash of its bytes, which is the root `iroh-blobs`
    /// gives the manifest as a blob.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::manifest::Manifest;
    ///
    /// let empty = Manifest::new(vec![], vec![])?;
    /// assert_eq!(empty.root(), *blake3::hash(&[1, 0, 0]).as_bytes());
    /// # Ok::<(), sluice_core::Error>(())
    /// ```
    pub fn root(&self) -> [u8; 32] {
        *blake3::hash(&self.encode()).as_bytes()
    }

    /// The entries, sorted by path.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::manifest::Manifest;
    ///
    /// assert!(Manifest::new(vec![], vec![])?.entries().is_empty());
    /// # Ok::<(), sluice_core::Error>(())
    /// ```
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// The packs that [`Loc::Packed`] entries point into.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::manifest::Manifest;
    ///
    /// assert!(Manifest::new(vec![], vec![])?.packs().is_empty());
    /// # Ok::<(), sluice_core::Error>(())
    /// ```
    pub fn packs(&self) -> &[PackRef] {
        &self.packs
    }
}
