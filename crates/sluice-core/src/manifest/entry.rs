//! One entry of a manifest, and the rules that look at that entry alone.

use serde::Serialize;

use super::link::LinkTarget;
use super::path::ManifestPath;
use super::rules::{Fault, rule};

/// The highest `mode` a manifest may carry: the nine permission bits and the setuid, setgid and
/// sticky bits. The file type bits above them belong to `kind`.
const MAX_MODE: u32 = 0o7777;

/// What an [`Entry`] is. The wire value is one byte.
///
/// # Examples
///
/// ```
/// use sluice_core::manifest::Kind;
///
/// assert_ne!(Kind::File, Kind::Dir);
/// assert_eq!(Kind::Symlink as u8, 2);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Kind {
    /// A regular file, wire value 0.
    File = 0,
    /// A folder, wire value 1. Every folder under the one sent has an entry, empty or not.
    Dir = 1,
    /// A symbolic link, wire value 2. Recorded and never followed.
    Symlink = 2,
}

/// Where the bytes of a file are. Only the order of the variants is the wire format: postcard
/// writes the position of the variant first, so `Blob` is 0, `Packed` is 1 and `None` is 2.
///
/// # Examples
///
/// ```
/// use sluice_core::manifest::Loc;
///
/// let alone = Loc::Blob { root: [7; 32] };
/// let inside = Loc::Packed { pack_index: 0, offset: 4096, len: 100 };
/// assert_ne!(alone, inside);
/// assert_ne!(inside, Loc::None);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum Loc {
    /// A blob of its own. `root` is the BLAKE3 hash of the content, which is the root
    /// `iroh-blobs` gives a raw blob.
    Blob {
        /// The BLAKE3 hash of the content.
        root: [u8; 32],
    },
    /// A byte range of a pack, which is itself a blob listed in [`PackRef`]s.
    Packed {
        /// Which pack, as a position in the manifest's pack list.
        pack_index: u32,
        /// Where the file starts in the pack.
        offset: u64,
        /// How many bytes the file takes. It equals the entry's `size`.
        len: u64,
    },
    /// No content: a folder or a symlink.
    None,
}

/// A pack blob that [`Loc::Packed`] entries point into.
///
/// # Examples
///
/// ```
/// use sluice_core::manifest::PackRef;
///
/// let pack = PackRef { root: [1; 32], size: 64 << 20 };
/// assert_eq!(pack.size, 67_108_864);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PackRef {
    /// The BLAKE3 root of the pack.
    pub root: [u8; 32],
    /// The size of the pack in bytes.
    pub size: u64,
}

/// A file, folder or symlink in a manifest.
///
/// The fields can be set freely. [`Manifest::new`](super::Manifest::new) is where an entry is
/// checked against the others, so a `Manifest` never holds one that breaks a rule.
///
/// # Examples
///
/// ```
/// use sluice_core::manifest::{Entry, Kind, Loc, path::ManifestPath};
///
/// let entry = Entry {
///     path: ManifestPath::parse("docs/readme.txt")?,
///     kind: Kind::File,
///     size: 12,
///     mtime_ms: 1_800_000_000_000,
///     mode: 0o644,
///     loc: Loc::Blob { root: [9; 32] },
///     link_target: None,
/// };
/// assert_eq!(entry.path.as_str(), "docs/readme.txt");
/// # Ok::<(), sluice_core::Error>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Relative to the folder that was sent, which is not an entry itself.
    pub path: ManifestPath,
    /// File, folder or symlink.
    pub kind: Kind,
    /// The size of the content in bytes. Zero for a folder and a symlink.
    pub size: u64,
    /// The modification time in UTC milliseconds. A time before 1970 is 0.
    pub mtime_ms: u64,
    /// Permission bits only, at most `0o7777`. Zero when unknown.
    pub mode: u32,
    /// Where the bytes are.
    pub loc: Loc,
    /// Where a symlink points. Present exactly when `kind` is [`Kind::Symlink`].
    pub link_target: Option<LinkTarget>,
}

impl Entry {
    /// The rules that need this entry and nothing else: link target presence, rule 5 of the link
    /// target rules, kind with `loc` and `size`, and `mode`. The rules that compare entries, and
    /// the ones that need the pack list, are in `rules.rs`.
    pub(super) fn check(&self) -> Result<(), Fault> {
        match (self.kind, &self.link_target) {
            (Kind::Symlink, Some(target)) => {
                if !target.stays_inside(&self.path) {
                    return Err(Fault::within(rule::LINK_TARGET, 5));
                }
            }
            (Kind::Symlink, None) | (_, Some(_)) => return Err(Fault::new(rule::LINK_PRESENCE)),
            (_, None) => {}
        }
        let agrees = match (self.kind, self.loc) {
            (Kind::File, Loc::Blob { .. } | Loc::Packed { .. }) => true,
            (Kind::Dir | Kind::Symlink, Loc::None) => self.size == 0,
            _ => false,
        };
        if !agrees {
            return Err(Fault::new(rule::KIND_LOC));
        }
        if self.mode > MAX_MODE {
            return Err(Fault::new(rule::MODE));
        }
        Ok(())
    }
}
