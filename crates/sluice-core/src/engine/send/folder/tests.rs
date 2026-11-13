//! A confined send copies what it read from a handle it proved, so a file swapped after the walk is
//! refused before any of its bytes are taken, and no more is read than the walk saw.

use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use iroh_blobs::{Hash, store::fs::FsStore};
use n0_future::StreamExt;

use super::import_blobs;
use crate::Error;
use crate::engine::open_folder;
use crate::engine::send::copy::{CHANGED, chunks};
use crate::manifest::{Build, Built, ConfinedArg, Manifest};

fn confined(folder: &Path) -> Built {
    let name = folder.file_name().unwrap().to_string_lossy().into_owned();
    let arg = ConfinedArg::folder(open_folder(folder).unwrap(), name);
    match Manifest::build_confined(vec![arg]).unwrap() {
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
    let cancel = Arc::new(AtomicBool::new(false));
    let result = import_blobs(&store, built, &built.manifest, &mut tags, &cancel).await;
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

/// Puts a file with other bytes under a name by renaming it over the old one, so the new file is
/// another object whatever the file system does with freed numbers.
fn rename_over(folder: &Path, name: &str, bytes: &[u8]) {
    let fresh = folder.join(format!("{name}.new"));
    fs::write(&fresh, bytes).unwrap();
    fs::rename(&fresh, folder.join(name)).unwrap();
}

fn never() -> Arc<AtomicBool> {
    Arc::new(AtomicBool::new(false))
}

fn text_of(result: Result<(), Error>) -> String {
    match result {
        Err(Error::Verify(text)) => text,
        other => panic!("expected a refusal as changed: {other:?}"),
    }
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
    let other = vec![9u8; 2 * 1024 * 1024];
    rename_over(&sent, "data.bin", &other);
    let (result, held) = import(&built, &other).await;
    assert!(text_of(result).contains("replaced"));
    assert!(!held, "the bytes of the replacement were not taken");
}

#[tokio::test]
async fn a_small_packed_file_replaced_by_one_of_the_same_size_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let sent = dir.path().join("sent");
    fs::create_dir(&sent).unwrap();
    fs::write(sent.join("tiny.txt"), b"tiny").unwrap();
    let built = confined(&sent);
    rename_over(&sent, "tiny.txt", b"fake");
    let pack = super::add_pack_for_test(&built, never()).await;
    assert!(text_of(pack).contains("replaced"));
}

#[tokio::test]
async fn a_file_that_grew_or_shrank_after_the_walk_is_refused_as_changed() {
    for resize in [3 * 1024 * 1024u64, 1024] {
        let dir = tempfile::tempdir().unwrap();
        let sent = folder_with_a_big_file(dir.path());
        let built = confined(&sent);
        let file = fs::OpenOptions::new()
            .write(true)
            .open(sent.join("data.bin"))
            .unwrap();
        file.set_len(resize).unwrap();
        drop(file);
        let (result, _) = import(&built, b"unrelated").await;
        assert!(text_of(result).contains("changed"), "resized to {resize}");
    }
}

/// A sparse file grown to a terabyte after the walk costs nothing to refuse and nothing is read.
#[cfg(unix)]
#[tokio::test]
async fn a_sparse_file_grown_to_a_terabyte_is_refused_without_reading_it() {
    let dir = tempfile::tempdir().unwrap();
    let sent = folder_with_a_big_file(dir.path());
    let built = confined(&sent);
    let file = fs::OpenOptions::new()
        .write(true)
        .open(sent.join("data.bin"))
        .unwrap();
    file.set_len(1 << 40).unwrap();
    drop(file);
    let (result, _) = import(&built, b"unrelated").await;
    assert!(text_of(result).contains("changed"));
}

/// The stream itself reads no more than the walk saw, for a file that grows after its size was
/// checked, and says so for one that comes up short.
#[tokio::test]
async fn the_copy_stops_at_the_size_the_walk_saw() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("f");
    fs::write(&path, vec![1u8; 4096]).unwrap();
    let mut grown = Box::pin(chunks(fs::File::open(&path).unwrap(), 1000, never()));
    let mut seen = 0;
    let mut failed = None;
    while let Some(item) = grown.next().await {
        match item {
            Ok(bytes) => seen += bytes.len(),
            Err(e) => failed = Some(e.to_string()),
        }
    }
    assert!(seen <= 1000, "{seen} bytes passed the size the walk saw");
    assert_eq!(failed.as_deref(), Some(CHANGED));

    let mut short = Box::pin(chunks(fs::File::open(&path).unwrap(), 5000, never()));
    let mut ended = None;
    while let Some(item) = short.next().await {
        ended = item.err().map(|e| e.to_string());
    }
    assert_eq!(ended.as_deref(), Some(CHANGED));
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

/// A single file sent by the confined path has the root a one-shot send of it has, so a receipt
/// for it verifies against the file.
#[test]
fn a_single_file_has_the_same_root_confined_or_not() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("one.bin");
    fs::write(&file, vec![5u8; 3000]).unwrap();
    let parent = open_folder(dir.path()).unwrap();
    let arg = ConfinedArg::file(parent, "one.bin".to_string());
    let Build::Ready(built) = Manifest::build_confined(vec![arg]).unwrap() else {
        panic!("a plain file builds");
    };
    let plain = crate::engine::root_of_path(&file).unwrap();
    assert_eq!(built.manifest.sequence_root(), plain.root);
}

/// A send that fails while it copies removes its store and its folder, and leaves nothing in the
/// home for the next command to find.
#[tokio::test]
async fn a_send_that_fails_while_it_copies_leaves_nothing_in_the_home() {
    let dir = tempfile::tempdir().unwrap();
    let home = crate::Home::open(&dir.path().join("home")).unwrap();
    let sent = folder_with_a_big_file(dir.path());
    let args = vec![ConfinedArg::folder(
        open_folder(&sent).unwrap(),
        "sent".to_string(),
    )];
    let swapped = sent.clone();
    crate::engine::send::copy::before_copy(move || rename_over(&swapped, "data.bin", b"other"));
    let result = super::prepare_paths_confined(
        &home,
        args,
        crate::engine::RelayModeChoice::Loopback,
        crate::engine::Congestion::Cubic,
        None,
        None,
        None,
        never(),
    )
    .await;
    crate::engine::send::copy::clear_before_copy();
    assert!(matches!(result, Err(Error::Verify(_))));
    crate::engine::wait_for_removals(std::time::Duration::from_secs(10));
    let left: Vec<_> = fs::read_dir(home.temporary_folder().unwrap())
        .unwrap()
        .filter_map(Result::ok)
        .collect();
    assert!(left.is_empty(), "left behind: {left:?}");
}

/// The room check is a decision on a free-space reading that a test can set: a send refuses when the
/// volume has less than it copies, says both numbers, and goes ahead at exactly enough.
#[test]
fn a_send_without_room_for_its_copies_is_refused_and_one_with_exactly_enough_is_not() {
    use crate::engine::send::copy::refuse_without_room;
    let refused = refuse_without_room(999, 1000).unwrap_err();
    let Error::Storage(text) = refused else {
        panic!("a lack of room is a storage error");
    };
    assert!(text.contains("999") && text.contains("1000"), "{text}");
    refuse_without_room(1000, 1000).unwrap();
    refuse_without_room(u64::MAX, 1000).unwrap();
}

/// A send that is told to stop ends as cancelled between files and between chunks, and a copy in
/// progress stops at the next chunk.
#[tokio::test]
async fn a_send_that_is_told_to_stop_ends_as_cancelled() {
    let dir = tempfile::tempdir().unwrap();
    let built = confined(&folder_with_a_big_file(dir.path()));
    let store_dir = tempfile::tempdir().unwrap();
    let store = FsStore::load(store_dir.path()).await.unwrap();
    let mut tags = Vec::new();
    let cancel = Arc::new(AtomicBool::new(true));
    let result = import_blobs(&store, &built, &built.manifest, &mut tags, &cancel).await;
    assert!(matches!(result, Err(Error::Cancelled)), "{result:?}");
    drop(tags);
    store.shutdown().await.unwrap();

    let path = dir.path().join("f");
    fs::write(&path, vec![1u8; 4096]).unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let mut stream = Box::pin(chunks(fs::File::open(&path).unwrap(), 4096, stop.clone()));
    stop.store(true, Ordering::Relaxed);
    let mut ended = None;
    while let Some(item) = stream.next().await {
        ended = item.err().map(|e| e.to_string());
    }
    assert_eq!(ended.as_deref(), Some(crate::engine::send::copy::STOPPED));
}

/// A send that is told to stop opens no file. The opens are counted through the hook, and the flag is
/// already set when the import and the packing start, so the count is zero for both.
#[tokio::test]
async fn a_send_told_to_stop_before_it_starts_opens_no_file() {
    use std::sync::atomic::AtomicUsize;
    static OPENS: AtomicUsize = AtomicUsize::new(0);
    OPENS.store(0, Ordering::SeqCst);
    crate::engine::send::copy::before_copy(|| {
        OPENS.fetch_add(1, Ordering::SeqCst);
    });

    let dir = tempfile::tempdir().unwrap();
    let blob = confined(&folder_with_a_big_file(dir.path()));
    let stopped = Arc::new(AtomicBool::new(true));
    let store_dir = tempfile::tempdir().unwrap();
    let store = FsStore::load(store_dir.path()).await.unwrap();
    let mut tags = Vec::new();
    let result = import_blobs(&store, &blob, &blob.manifest, &mut tags, &stopped).await;
    assert!(matches!(result, Err(Error::Cancelled)), "{result:?}");
    drop(tags);
    store.shutdown().await.unwrap();

    let small = dir.path().join("small");
    fs::create_dir(&small).unwrap();
    fs::write(small.join("tiny.txt"), b"tiny").unwrap();
    let packed = confined(&small);
    let result = super::add_pack_for_test(&packed, stopped.clone()).await;
    assert!(matches!(result, Err(Error::Cancelled)), "{result:?}");

    crate::engine::send::copy::clear_before_copy();
    assert_eq!(OPENS.load(Ordering::SeqCst), 0, "no file was opened");
}
