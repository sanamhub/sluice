//! A confined send copies what it read from a handle it proved, so a file swapped after the walk is
//! refused before any of its bytes are taken.

use std::fs;
use std::path::Path;

use iroh_blobs::{Hash, store::fs::FsStore};

use super::import_blobs;
use crate::Error;
use crate::manifest::{Build, Built, Manifest};

fn confined(folder: &Path) -> Built {
    match Manifest::build_paths_confined(&[folder.to_path_buf()]).unwrap() {
        Build::Ready(built) => built,
        Build::Refused(_) => panic!("the folder is built"),
    }
}

/// Imports the folder's blobs and says what happened, and whether the bytes of `outside` are in the
/// store afterwards.
async fn import(built: &Built, outside: &[u8]) -> (Result<(), Error>, bool) {
    let dir = tempfile::tempdir().unwrap();
    let store = FsStore::load(dir.path()).await.unwrap();
    let mut tags = Vec::new();
    let result = import_blobs(&store, built, &built.manifest, &mut tags).await;
    let held = store.blobs().has(Hash::new(outside)).await.unwrap();
    drop(tags);
    store.shutdown().await.unwrap();
    (result, held)
}

fn folder_with_a_big_file(dir: &Path) -> std::path::PathBuf {
    let sent = dir.join("sent");
    fs::create_dir(&sent).unwrap();
    // Over the pack threshold, so it is a blob of its own and not part of a pack.
    fs::write(sent.join("data.bin"), vec![7u8; 2 * 1024 * 1024]).unwrap();
    sent
}

#[tokio::test]
async fn a_file_that_is_still_the_walked_object_is_imported() {
    let dir = tempfile::tempdir().unwrap();
    let built = confined(&folder_with_a_big_file(dir.path()));
    let (result, _) = import(&built, b"unrelated").await;
    result.unwrap();
}

#[tokio::test]
async fn a_file_replaced_after_the_walk_is_refused_and_none_of_the_new_bytes_are_taken() {
    let dir = tempfile::tempdir().unwrap();
    let sent = folder_with_a_big_file(dir.path());
    let built = confined(&sent);
    // A different file takes the name: another object, with other bytes.
    let outside = vec![9u8; 2 * 1024 * 1024];
    // It is written beside the first and renamed over it, so both exist at once and the system
    // cannot give the new file the identity of the one it replaces.
    fs::write(sent.join("data.new"), &outside).unwrap();
    fs::rename(sent.join("data.new"), sent.join("data.bin")).unwrap();
    let (result, held) = import(&built, &outside).await;
    let Err(Error::Verify(text)) = result else {
        panic!("the send is refused as changed: {result:?}");
    };
    assert!(text.contains("replaced"), "{text}");
    assert!(!held, "the bytes of the replacement were not taken");
}

/// The case a link makes possible: the name becomes a link to a file outside the folder.
#[cfg(unix)]
#[tokio::test]
async fn a_file_swapped_for_a_link_to_a_file_outside_is_refused_and_its_bytes_are_not_taken() {
    let dir = tempfile::tempdir().unwrap();
    let sent = folder_with_a_big_file(dir.path());
    let built = confined(&sent);
    let outside_bytes = vec![9u8; 2 * 1024 * 1024];
    let outside = dir.path().join("outside.bin");
    fs::write(&outside, &outside_bytes).unwrap();
    fs::remove_file(sent.join("data.bin")).unwrap();
    std::os::unix::fs::symlink(&outside, sent.join("data.bin")).unwrap();
    let (result, held) = import(&built, &outside_bytes).await;
    assert!(matches!(result, Err(Error::Verify(_))), "{result:?}");
    assert!(!held, "no byte of the outside file was taken");
}

#[tokio::test]
async fn a_small_packed_file_replaced_after_the_walk_is_refused_too() {
    let dir = tempfile::tempdir().unwrap();
    let sent = dir.path().join("sent");
    fs::create_dir(&sent).unwrap();
    fs::write(sent.join("tiny.txt"), b"tiny").unwrap();
    let built = confined(&sent);
    fs::remove_file(sent.join("tiny.txt")).unwrap();
    fs::write(sent.join("tiny.txt"), b"other").unwrap();
    let pack = super::add_pack_for_test(&built).await;
    assert!(matches!(pack, Err(Error::Verify(_))), "{pack:?}");
}

/// A single file sent by the confined path has the root a one-shot send of it has, so a receipt
/// for it verifies against the file.
#[test]
fn a_single_file_has_the_same_root_confined_or_not() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("one.bin");
    fs::write(&file, vec![5u8; 3000]).unwrap();
    let confined = confined_of(&file);
    let plain = crate::engine::root_of_path(&file).unwrap();
    assert_eq!(confined.manifest.sequence_root(), plain.root);
}

fn confined_of(path: &Path) -> Built {
    confined(path)
}
