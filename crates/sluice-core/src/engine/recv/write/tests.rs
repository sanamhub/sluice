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
        ours: std::collections::BTreeSet::new(),
    };
    write(manifest, &plan, store, &options(out), partial, false, &tx)
        .await
        .unwrap();
}

/// A store that is kept has to keep its own bytes. A file that took them by link is one inode under
/// two names while the store lives, so an edit to the installed file would change the blob a resume
/// reads; the writer copies the store's bytes back out when a failure leaves the store behind.
#[tokio::test]
async fn a_kept_store_is_given_its_own_bytes_back() {
    use super::same_file;
    let dir = tempfile::tempdir().unwrap();
    let partial = dir.path().join("store");
    let store = FsStore::load(&partial).await.unwrap();
    // Large enough that the store keeps the bytes in a file rather than in its metadata, which is
    // what makes a link possible at all.
    let one = vec![1u8; 1 << 20];
    let two = vec![2u8; 8 << 20];
    let root_one = *store.add_slice(&one).await.unwrap().hash.as_bytes();
    let root_two = *store.add_slice(&two).await.unwrap().hash.as_bytes();
    let manifest = Manifest::new(
        vec![
            file("a.bin", root_one, one.len() as u64),
            file("b.bin", root_two, two.len() as u64),
        ],
        vec![],
    )
    .unwrap();
    let out = dir.path().join("out");
    fs::create_dir_all(&out).unwrap();
    let plan = Plan {
        files: 2,
        bytes: (one.len() + two.len()) as u64,
        ours: std::collections::BTreeSet::new(),
    };
    let mut options = options(&out);
    // The first file fits and the second does not, so the transfer fails with the store kept.
    options.write_limit = Some(2 << 20);
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let err = write(&manifest, &plan, &store, &options, &partial, false, &tx)
        .await
        .unwrap_err();
    assert!(matches!(err, crate::Error::Storage(_)), "{err}");

    let data = iroh_blobs::store::fs::options::Options::new(&partial)
        .path
        .data_path(&iroh_blobs::Hash::from(root_one));
    let written = out.join("a.bin");
    assert!(
        !same_file(&data, &written),
        "the kept store still shares its bytes with the file"
    );
    assert_eq!(fs::read(&data).unwrap(), one);
    fs::write(&written, b"the user's own edit of the file").unwrap();
    assert_eq!(
        fs::read(&data).unwrap(),
        one,
        "the store followed the user's edit"
    );
    store.shutdown().await.unwrap();
}

/// A received file always keeps owner read and write. Without them the receive that wrote it could
/// not read it back to compare on a resume, so a sender mode of `000` arrives as `600` and a
/// read-only `444` arrives as `644`; on Windows neither makes the file read-only.
#[tokio::test]
async fn a_mode_without_owner_access_keeps_owner_read_and_write() {
    let dir = tempfile::tempdir().unwrap();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    for given in [0o000, 0o444, 0o750] {
        let path = dir.path().join(format!("mode-{given:o}.bin"));
        fs::write(&path, b"bytes").unwrap();
        let file = std::fs::File::options().write(true).open(&path).unwrap();
        super::file::apply(
            &file,
            &path,
            super::file::Meta {
                mtime_ms: 0,
                mode: given,
            },
            true,
            &tx,
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let expected = given | 0o600;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                expected,
                "mode {given:o}"
            );
        }
        #[cfg(windows)]
        assert!(
            !fs::metadata(&path).unwrap().permissions().readonly(),
            "mode {given:o}"
        );
    }
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
