//! The disk-backed provider for `serve --store fs`: a generated file, referenced in place by an
//! `iroh-blobs` file store, so a 100 GB transfer is limited by the disk and not by RAM.

use std::path::Path;

use anyhow::{Context, Result, anyhow, ensure};
use iroh_blobs::{
    BlobFormat, Hash,
    api::{
        blobs::{AddPathOptions, ImportMode},
        proto::BlobStatus,
    },
    store::fs::FsStore,
};

use crate::data;

/// What `serve` needs to answer from disk.
pub struct Provider {
    /// The open store. It has to outlive the router.
    pub store: FsStore,
    /// BLAKE3 root of the data file.
    pub hash: Hash,
}

/// Generates the data file if it is missing or the wrong size, then makes sure the store at
/// `<dir>/store` holds it. All of it happens before the ticket is printed, so none of it is in
/// anything `fetch` measures. Later runs with the same seed and size reuse both the file and the
/// import, and skip the hashing pass, which for 100 GB is minutes.
pub async fn open(dir: &Path, seed: u64, size: u64) -> Result<Provider> {
    // The store refuses a relative path.
    let dir = std::path::absolute(dir).context("resolve --dir")?;
    std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;

    let file = dir.join(format!("data-{seed}-{size}.bin"));
    let present = std::fs::metadata(&file).is_ok_and(|m| m.len() == size);
    if !present {
        tracing::info!(path = %file.display(), size, "generating the data file");
        let target = file.clone();
        tokio::task::spawn_blocking(move || data::write_file(seed, size, &target))
            .await
            .context("data file task panicked")??;
    }

    let store_dir = dir.join("store");
    let store = FsStore::load(&store_dir)
        .await
        .map_err(|e| anyhow!("open store: {e}"))?;
    let tag = format!("probe-{seed}-{size}");

    // A regenerated file is imported again, whatever the tag says.
    let known = if present {
        known_hash(&store, &tag, size).await?
    } else {
        None
    };
    let hash = match known {
        Some(hash) => hash,
        None => {
            tracing::info!(path = %file.display(), "importing the data file in place");
            let options = AddPathOptions {
                path: file,
                format: BlobFormat::Raw,
                // Reference the file where it is. `Copy` would double a 100 GB file on disk.
                mode: ImportMode::TryReference,
            };
            let added = store
                .blobs()
                .add_path_with_opts(options)
                .with_named_tag(&tag)
                .await
                .map_err(|e| anyhow!("import: {e}"))?;
            added.hash
        }
    };

    // A store may ignore TryReference. Say so, because the copy would be 100 GB of disk.
    let copy = store_dir
        .join("data")
        .join(format!("{}.data", hash.to_hex()));
    ensure!(
        !copy.exists(),
        "the store copied the data file to {} instead of referencing it",
        copy.display()
    );
    Ok(Provider { store, hash })
}

/// The root a previous run imported under `tag`, if the store still holds all of it.
async fn known_hash(store: &FsStore, tag: &str, size: u64) -> Result<Option<Hash>> {
    let Some(info) = store
        .tags()
        .get(tag)
        .await
        .map_err(|e| anyhow!("read tag: {e}"))?
    else {
        return Ok(None);
    };
    let status = store
        .blobs()
        .status(info.hash)
        .await
        .map_err(|e| anyhow!("read blob status: {e}"))?;
    Ok(match status {
        BlobStatus::Complete { size: held } if held == size => Some(info.hash),
        _ => None,
    })
}
