//! Reading the checksums a release carries: one `.sha256` file beside each artifact, whose first
//! word is the hash and whose second is the artifact's name, as `sha256sum` writes them.

use std::{collections::BTreeMap, path::Path};

use anyhow::Result;

use super::Checksums;

/// Every checksum in `dir`, by artifact name. The folder is the release's own `.sha256` assets,
/// which `cargo xtask package --from` reads so that a maintainer never edits a hash by hand.
pub fn read(dir: &Path) -> Result<Checksums> {
    let entries = std::fs::read_dir(dir)
        .map_err(|e| anyhow::anyhow!("cannot list {}: {e}", dir.display()))?;
    let mut sums = BTreeMap::new();
    for entry in entries {
        let path = entry
            .map_err(|e| anyhow::anyhow!("cannot list {}: {e}", dir.display()))?
            .path();
        if path.extension().is_none_or(|ext| ext != "sha256") {
            continue;
        }
        let text = std::fs::read_to_string(&path)
            .map_err(|e| anyhow::anyhow!("cannot read {}: {e}", path.display()))?;
        for line in text.lines() {
            let mut words = line.split_whitespace();
            let (Some(hash), Some(name)) = (words.next(), words.next()) else {
                continue;
            };
            if hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                anyhow::bail!("{} is not a SHA-256 checksum, in {}", hash, path.display());
            }
            // A line may name a path, and a leading `*` marks a binary read.
            let name = name.trim_start_matches('*');
            let name = name.rsplit(['/', '\\']).next().unwrap_or(name);
            let hash = hash.to_ascii_lowercase();
            match sums.entry(name.to_string()) {
                std::collections::btree_map::Entry::Occupied(seen) => {
                    // The same artifact in two files is fine when they agree; two hashes for it
                    // mean the release is not one release, and either choice could publish a pin
                    // that does not match the bytes.
                    if seen.get() != &hash {
                        anyhow::bail!("{} has two checksums: {} and {}", name, seen.get(), hash);
                    }
                }
                std::collections::btree_map::Entry::Vacant(entry) => {
                    entry.insert(hash);
                }
            }
        }
    }
    if sums.is_empty() {
        anyhow::bail!("{} holds no `.sha256` file", dir.display());
    }
    Ok(sums)
}

#[cfg(test)]
mod tests {
    use super::read;

    /// A `.sha256` file is read by name, with its hash lower-cased, whatever prefix the tool that
    /// wrote it used.
    #[test]
    fn a_checksum_file_is_read_by_artifact_name() {
        let dir = tempfile::tempdir().unwrap();
        let hash = "0123456789ABCDEF".repeat(4);
        std::fs::write(
            dir.path()
                .join("sluice-x86_64-unknown-linux-gnu.tar.xz.sha256"),
            format!("{hash}  ./sluice-x86_64-unknown-linux-gnu.tar.xz\n"),
        )
        .unwrap();
        std::fs::write(
            dir.path().join("sluice-x86_64-pc-windows-msvc.zip.sha256"),
            format!("{hash} *sluice-x86_64-pc-windows-msvc.zip\n"),
        )
        .unwrap();
        let sums = read(dir.path()).unwrap();
        assert_eq!(sums.len(), 2);
        assert_eq!(
            sums["sluice-x86_64-unknown-linux-gnu.tar.xz"],
            hash.to_ascii_lowercase()
        );
        assert_eq!(
            sums["sluice-x86_64-pc-windows-msvc.zip"],
            hash.to_ascii_lowercase()
        );
    }

    /// A folder with nothing to read is a refusal that names it, not an empty set of files.
    #[test]
    fn a_folder_without_checksums_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let err = read(dir.path()).unwrap_err().to_string();
        assert!(err.contains("no `.sha256` file"), "{err}");
    }

    /// Two files that give one artifact different hashes are a release that disagrees with itself.
    /// Taking whichever line was read last would publish a pin that may not match the bytes.
    #[test]
    fn one_artifact_with_two_hashes_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let artifact = "sluice-cli-x86_64-unknown-linux-gnu.tar.xz";
        let first = "a".repeat(64);
        let second = "b".repeat(64);
        std::fs::write(
            dir.path().join("one.sha256"),
            format!("{first}  {artifact}\n"),
        )
        .unwrap();
        std::fs::write(
            dir.path().join("two.sha256"),
            format!("{second}  ./{artifact}\n"),
        )
        .unwrap();
        let err = read(dir.path()).unwrap_err().to_string();
        assert!(err.contains(artifact), "{err}");
        assert!(err.contains("two checksums"), "{err}");

        // The same hash twice is one release saying one thing twice.
        std::fs::write(
            dir.path().join("two.sha256"),
            format!("{first}  ./{artifact}\n"),
        )
        .unwrap();
        let sums = read(dir.path()).unwrap();
        assert_eq!(sums[artifact], first);
    }
}
