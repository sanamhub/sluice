//! Tests for writing a checked tree: a root that several files name, and one that keeps its
//! store copy.

use super::{Plan, write};
use crate::{
    engine::ReceiveOptions,
    manifest::{Entry, Kind, Loc, Manifest, path::ManifestPath},
};
use iroh_blobs::{api::proto::BlobStatus, store::fs::FsStore};
use std::fs;

fn file(name: &str, root: [u8; 32], size: u64) -> Entry {
    Entry {
        path: ManifestPath::parse(name).unwrap(),
        kind: Kind::File,
        size,
        mtime_ms: 0,
        mode: 0,
        loc: Loc::Blob { root },
        link_target: None,
    }
}

fn options(out: &std::path::Path) -> ReceiveOptions {
    ReceiveOptions {
        out: out.to_path_buf(),
        force: false,
        no_times: true,
        links: false,
        allow_hidden: false,
        limits: crate::manifest::Limits::default(),
        free_bytes: None,
        congestion: crate::engine::Congestion::Cubic,
        rate: None,
        write_limit: None,
        max_network_failures: crate::queue::MAX_NETWORK_FAILURES,
    }
}

async fn run(
    manifest: &Manifest,
    store: &FsStore,
    partial: &std::path::Path,
    out: &std::path::Path,
) {
    fs::create_dir_all(out).unwrap();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let plan = Plan {
        files: manifest.entries().len() as u64,
        bytes: manifest.entries().iter().map(|entry| entry.size).sum(),
    };
    write(manifest, plan, store, &options(out), partial, false, &tx)
        .await
        .unwrap();
}

/// A root that two files name is copied out once per file: taking the store's file for the
/// first would leave the second with no bytes to read.
#[tokio::test]
async fn two_files_with_one_root_are_both_written() {
    let dir = tempfile::tempdir().unwrap();
    let partial = dir.path().join("store");
    let store = FsStore::load(&partial).await.unwrap();
    let body = vec![7u8; 2 << 20];
    let hash = store.add_slice(&body).await.unwrap().hash;
    let root = *hash.as_bytes();
    let manifest = Manifest::new(
        vec![
            file("a.bin", root, body.len() as u64),
            file("b.bin", root, body.len() as u64),
        ],
        vec![],
    )
    .unwrap();
    let out = dir.path().join("out");
    run(&manifest, &store, &partial, &out).await;
    assert_eq!(fs::read(out.join("a.bin")).unwrap(), body);
    assert_eq!(fs::read(out.join("b.bin")).unwrap(), body);
    // The store still holds a readable copy; it did not hand its only one to the first file.
    let data = iroh_blobs::store::fs::options::Options::new(&partial)
        .path
        .data_path(&hash);
    assert!(data.exists(), "the store's data file was moved away");
    assert!(matches!(
        store.blobs().status(hash).await.unwrap(),
        BlobStatus::Complete { .. }
    ));
    store.shutdown().await.unwrap();
}

/// One file that names a root keeps the store's file in place and gives the target a second
/// name for it, so a single large file never needs twice its size on disk.
#[tokio::test]
async fn one_file_takes_a_link_and_leaves_the_store_readable() {
    let dir = tempfile::tempdir().unwrap();
    let partial = dir.path().join("store");
    let store = FsStore::load(&partial).await.unwrap();
    let body = vec![3u8; 2 << 20];
    let hash = store.add_slice(&body).await.unwrap().hash;
    let manifest = Manifest::new(
        vec![file("one.bin", *hash.as_bytes(), body.len() as u64)],
        vec![],
    )
    .unwrap();
    let out = dir.path().join("out");
    run(&manifest, &store, &partial, &out).await;
    let target = out.join("one.bin");
    assert_eq!(fs::read(&target).unwrap(), body);
    let data = iroh_blobs::store::fs::options::Options::new(&partial)
        .path
        .data_path(&hash);
    assert!(data.exists(), "the store's data file was moved away");
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let target = fs::metadata(&target).unwrap();
        let stored = fs::metadata(&data).unwrap();
        assert_eq!((target.dev(), target.ino()), (stored.dev(), stored.ino()));
    }
    store.shutdown().await.unwrap();
}
