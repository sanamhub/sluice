//! The hash sequence a static ticket names, as `SPEC.md` section 8 states it.
//!
//! A Static ticket with format 1 does not name the manifest root directly. It names a blob whose
//! content is 32-byte roots: the manifest root, then the pack roots in pack order, then every
//! distinct `Blob` root of a `File` entry in manifest order. The receiver compares the sequence it
//! fetched with the one its decoded manifest yields, so a sequence with a root added, dropped,
//! repeated or moved is refused.

use std::collections::BTreeSet;

use crate::{Error, manifest::Limits};

use super::{Kind, Loc, Manifest};

impl Manifest {
    /// The roots a static ticket must name, in order. The first is [`Manifest::root`].
    ///
    /// # Examples
    /// ```
    /// use sluice_core::manifest::Manifest;
    /// # use sluice_core::manifest::{Entry, Kind, Loc, path::ManifestPath};
    /// # let file = |path: &str, root: [u8; 32]| -> Result<Entry, sluice_core::Error> {
    /// #     Ok(Entry {
    /// #         path: ManifestPath::parse(path)?, kind: Kind::File, size: 5,
    /// #         mtime_ms: 0, mode: 0o644, loc: Loc::Blob { root }, link_target: None,
    /// #     })
    /// # };
    /// # let manifest = Manifest::new(vec![file("a", [7; 32])?, file("b", [7; 32])?], vec![])?;
    /// let sequence = manifest.hash_sequence();
    /// assert_eq!(sequence.len(), 2);
    /// assert_eq!(sequence[0], manifest.root());
    /// assert_eq!(sequence[1], [7; 32]);
    /// # Ok::<(), sluice_core::Error>(())
    /// ```
    pub fn hash_sequence(&self) -> Vec<[u8; 32]> {
        let mut sequence = vec![self.root()];
        sequence.extend(self.packs.iter().map(|pack| pack.root));
        let mut seen = BTreeSet::new();
        for entry in &self.entries {
            if entry.kind == Kind::File
                && let Loc::Blob { root } = entry.loc
                && seen.insert(root)
            {
                sequence.push(root);
            }
        }
        sequence
    }

    /// The bytes of the sequence blob, which is what a sender adds to its store and a ticket's
    /// root names.
    ///
    /// # Examples
    /// ```
    /// use sluice_core::manifest::Manifest;
    /// let empty = Manifest::new(vec![], vec![])?;
    /// assert_eq!(empty.sequence_bytes(), empty.root().to_vec());
    /// # Ok::<(), sluice_core::Error>(())
    /// ```
    pub fn sequence_bytes(&self) -> Vec<u8> {
        self.hash_sequence()
            .into_iter()
            .flat_map(|root| root.to_vec())
            .collect()
    }

    /// The BLAKE3 root of [`Manifest::sequence_bytes`], which is the ticket's `root`.
    ///
    /// # Examples
    /// ```
    /// use sluice_core::manifest::Manifest;
    /// let empty = Manifest::new(vec![], vec![])?;
    /// assert_eq!(empty.sequence_root(), *blake3::hash(&empty.root()).as_bytes());
    /// # Ok::<(), sluice_core::Error>(())
    /// ```
    pub fn sequence_root(&self) -> [u8; 32] {
        *blake3::hash(&self.sequence_bytes()).as_bytes()
    }
}

/// Reads a sequence blob into its roots. It must be a whole number of 32-byte roots, at least the
/// manifest root, and no longer than `limits` allow.
pub fn parse(bytes: &[u8], limits: Limits) -> Result<Vec<[u8; 32]>, Error> {
    if bytes.is_empty() || !bytes.len().is_multiple_of(32) {
        return Err(Error::Policy(
            "the hash sequence is not a whole number of 32-byte roots".to_string(),
        ));
    }
    limits.check_bytes(bytes.len() as u64)?;
    let (chunks, _) = bytes.as_chunks::<32>();
    let roots = chunks.to_vec();
    Ok(roots)
}

#[cfg(test)]
mod tests {
    use super::parse;
    use crate::manifest::Limits;

    #[test]
    fn a_sequence_of_roots_is_read_back_in_order() {
        let mut bytes = vec![0u8; 64];
        bytes[32] = 9;
        let roots = parse(&bytes, Limits::default()).unwrap();
        assert_eq!(roots.len(), 2);
        assert_eq!(roots[0], [0; 32]);
        assert_eq!(roots[1][0], 9);
    }

    #[test]
    fn a_cut_or_empty_sequence_is_refused() {
        assert!(parse(&[], Limits::default()).is_err());
        assert!(parse(&[0u8; 31], Limits::default()).is_err());
    }
}
