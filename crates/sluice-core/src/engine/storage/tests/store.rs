//! The store types: the collector, closing a store on every way out, and giving a store its own
//! bytes back after a command was cancelled.

use std::time::Duration;

use crate::engine::storage::{
    self, Collector, Store, TempStore, data_path, name_count, open_store_after,
};

/// Only the two live stores collect, so a blob in a store that does not collect stays until that
/// store is closed or removed. In a live store a segment's tag is dropped as soon as the segment is
/// written out, and the bytes go at the next sweep instead of staying until the process ends.
#[tokio::test]
async fn a_blob_stays_while_a_tag_holds_it_and_goes_after_the_tag_is_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let store = open_store_after(dir.path(), Duration::from_millis(50))
        .await
        .unwrap();
    let body = vec![1u8; 2 << 20];
    let tag = store.blobs().add_slice(&body).temp_tag().await.unwrap();
    let hash = tag.hash();
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(
        store.blobs().has(hash).await.unwrap(),
        "a held blob was collected"
    );
    drop(tag);
    let mut gone = false;
    for _ in 0..60 {
        tokio::time::sleep(Duration::from_millis(50)).await;
        if !store.blobs().has(hash).await.unwrap() {
            gone = true;
            break;
        }
    }
    assert!(gone, "a blob no tag holds was kept by the collector");
    store.shutdown().await.unwrap();
}

/// A live stream's segments leave the store at the next sweep, so the whole bound on a live send is
/// one sweep's worth of what it sealed, and the sweep is what the constant says it is.
#[test]
fn the_collector_sweeps_every_ten_seconds() {
    assert_eq!(storage::GC_INTERVAL, Duration::from_secs(10));
}

/// A command that is cancelled has no future left to await a close with, and a store it leaves
/// behind is handed to the next command in this process: a resume opens the same folder, and it
/// would find the database still held by a task that was still using it.
#[tokio::test]
async fn a_store_a_cancelled_command_left_open_is_closed_by_the_next_open() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("store");
    let guard = Store::open(&root, Collector::Off).await.unwrap();
    // A task that was serving the store when the command was cancelled holds one of these.
    let held = guard.handle();
    drop(guard);

    let again = Store::open(&root, Collector::Off).await.unwrap();
    again.close().await.unwrap();
    drop(held);
}

/// A store of one receive, with `body` filed under its root, closed so nothing but the folder holds
/// the bytes.
async fn store_with(root: &std::path::Path, body: &[u8]) -> iroh_blobs::Hash {
    let store = Store::open(root, Collector::Off).await.unwrap();
    let tag = store.blobs().add_slice(body).temp_tag().await.unwrap();
    let hash = tag.hash();
    drop(tag);
    store.close().await.unwrap();
    hash
}

/// A blob of the store is hard-linked to a file in the output when a receive is cancelled between a
/// file being linked into place and the store being given its own copy back. The next claim copies
/// the bytes to a name of the store's own, so the output file and the store's data file no longer
/// name one inode, and the output keeps the bytes it had.
#[tokio::test]
async fn a_claim_repairs_what_a_killed_command_left() {
    let dir = tempfile::tempdir().unwrap();
    let partial = dir.path().join("store");
    let body = vec![3u8; 1 << 20];
    let hash = store_with(&partial, &body).await;

    let source = data_path(&partial, &hash);
    let shared = dir.path().join("shared.bin");
    std::fs::hard_link(&source, &shared).unwrap();
    // The copy a kill can leave, and one that no blob names.
    let leftover = source.with_extension("store");
    std::fs::write(&leftover, b"half").unwrap();
    let stray = source.parent().unwrap().join("nothing.store");
    std::fs::write(&stray, b"x").unwrap();

    let before = storage::identify(&partial).unwrap();
    storage::repair(&partial, before).await.unwrap();
    assert!(!leftover.exists(), "a half-written copy was kept");
    assert!(!stray.exists(), "a copy nothing names was kept");
    let names = name_count(&source, &std::fs::metadata(&source).unwrap()).unwrap();
    assert_eq!(names, 1, "the data file still shares its name");
    assert_eq!(std::fs::read(&source).unwrap(), body);
    assert_eq!(std::fs::read(&shared).unwrap(), body, "the output changed");
}

/// The same repair on a file the user edited: the bytes no longer hash to the blob, so they are
/// left where they are and the store's check is what refuses the blob.
#[tokio::test]
async fn a_source_edited_outside_the_store_is_not_installed() {
    let dir = tempfile::tempdir().unwrap();
    let partial = dir.path().join("store");
    let body = vec![5u8; 1 << 20];
    let hash = store_with(&partial, &body).await;
    let source = data_path(&partial, &hash);
    let shared = dir.path().join("edited.bin");
    std::fs::hard_link(&source, &shared).unwrap();
    std::fs::write(&shared, vec![6u8; body.len()]).unwrap();

    let before = storage::identify(&partial).unwrap();
    storage::repair(&partial, before).await.unwrap();
    let names = name_count(&source, &std::fs::metadata(&source).unwrap()).unwrap();
    assert_eq!(names, 2, "an edited file was copied into the store");
    assert_eq!(std::fs::read(&source).unwrap(), vec![6u8; body.len()]);
}

/// A link planted at the name the copy is written under is never written through: the copy opens a
/// name that must not be there yet and refuses to follow a link, so the file the link points at is
/// left exactly as it was.
#[cfg(unix)]
#[tokio::test]
async fn a_link_at_the_copy_name_is_not_written_through() {
    let dir = tempfile::tempdir().unwrap();
    let partial = dir.path().join("store");
    let body = vec![9u8; 64 * 1024];
    let hash = store_with(&partial, &body).await;
    let source = data_path(&partial, &hash);
    let shared = dir.path().join("shared.bin");
    std::fs::hard_link(&source, &shared).unwrap();
    // The link a swapper can plant between the sweep of half-written copies and the copy itself.
    let outside = dir.path().join("outside.bin");
    std::fs::write(&outside, b"untouched").unwrap();
    let planted = source.with_extension("store");
    std::os::unix::fs::symlink(&outside, &planted).unwrap();

    let before = storage::identify(&partial).unwrap();
    storage::repair(&partial, before).await.unwrap();
    assert_eq!(
        std::fs::read(&outside).unwrap(),
        b"untouched",
        "the store's bytes were written through a link"
    );
    // The repair still gave the store its own name: the link is gone, and the data file is alone.
    assert!(!planted.exists(), "the planted link was left");
    let names = name_count(&source, &std::fs::metadata(&source).unwrap()).unwrap();
    assert_eq!(names, 1, "the data file still shares its name");
    assert_eq!(std::fs::read(&source).unwrap(), body);
}

/// A folder swapped in under the store's name after it was checked is not repaired at all: the
/// identity of the folder under the name no longer matches the one the caller read, and a repair on
/// someone else's folder would copy through names they chose.
#[tokio::test]
async fn a_swapped_folder_is_refused_before_a_file_in_it_changes() {
    let dir = tempfile::tempdir().unwrap();
    let partial = dir.path().join("store");
    let body = vec![4u8; 1 << 20];
    let hash = store_with(&partial, &body).await;
    let before = storage::identify(&partial).unwrap();

    // The swap: the store is renamed away and a folder of another shape takes its name.
    let moved = dir.path().join("moved");
    std::fs::rename(&partial, &moved).unwrap();
    std::fs::create_dir(&partial).unwrap();
    std::fs::create_dir(partial.join("data")).unwrap();
    let planted = partial.join("data").join("planted.data");
    std::fs::write(&planted, b"planted").unwrap();
    let leftover = partial.join("data").join("planted.store");
    std::fs::write(&leftover, b"half").unwrap();

    let err = storage::repair(&partial, before).await.unwrap_err();
    assert!(matches!(err, crate::Error::Policy(_)), "{err}");
    assert_eq!(
        std::fs::read(&planted).unwrap(),
        b"planted",
        "a file in the swapped folder was changed"
    );
    assert!(
        leftover.exists(),
        "a file in the swapped folder was removed"
    );
    assert_eq!(std::fs::read(data_path(&moved, &hash)).unwrap(), body);
}

/// The store gives a blob its own bytes back by copying the shared file. Another process can edit
/// that shared file through the output while the copy runs, so the copy is judged by the bytes it
/// wrote: a copy that is not the blob is thrown away, the data file is left shared, and the export
/// check refuses it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_source_edited_while_it_is_copied_is_not_installed() {
    use std::{
        io::{Seek, SeekFrom, Write},
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
    };
    let dir = tempfile::tempdir().unwrap();
    let partial = dir.path().join("store");
    let mut body = vec![0u8; 16 << 20];
    for (at, byte) in body.iter_mut().enumerate() {
        *byte = (at % 251) as u8;
    }
    let hash = store_with(&partial, &body).await;
    let source = data_path(&partial, &hash);
    let shared = dir.path().join("shared.bin");
    std::fs::hard_link(&source, &shared).unwrap();

    let stop = Arc::new(AtomicBool::new(false));
    let writer = std::thread::spawn({
        let stop = Arc::clone(&stop);
        let shared = shared.clone();
        move || {
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .open(&shared)
                .unwrap();
            let chunk = [0xa5u8; 64 * 1024];
            while !stop.load(Ordering::Relaxed) {
                file.seek(SeekFrom::Start(0)).unwrap();
                file.write_all(&chunk).unwrap();
            }
        }
    });
    let before = storage::identify(&partial).unwrap();
    storage::repair(&partial, before).await.unwrap();
    stop.store(true, Ordering::Relaxed);
    writer.join().unwrap();

    let names = name_count(&source, &std::fs::metadata(&source).unwrap()).unwrap();
    if names == 1 {
        let installed = std::fs::read(&source).unwrap();
        assert_eq!(
            iroh_blobs::Hash::new(&installed),
            hash,
            "the store installed bytes that are not the blob"
        );
    }
}

/// An open that starts while another is closing a store it was handed waits for that close before
/// it opens its own. A retry of a receive asks for the folder whose store a cancelled command left,
/// and a store being closed still holds its database: without the wait, the retry finds the parked
/// list empty, opens the same database, and is refused for a lock nobody is going to keep.
#[tokio::test]
async fn an_open_waits_for_a_close_that_is_still_running() {
    let dir = tempfile::tempdir().unwrap();
    let waiting = dir.path().join("waiting");
    let retry = dir.path().join("retry");
    // The store a cancelled command left open, in the folder the retry asks for.
    let cancelled = Store::open(&retry, Collector::Off).await.unwrap();
    let held = cancelled.handle();
    drop(cancelled);

    // Both opens are polled on one thread in this order, which is the order the race was lost in:
    // the first takes the parked store and starts closing it, the second asks for that folder.
    let (first, second) = tokio::join!(
        Store::open(&waiting, Collector::Off),
        Store::open(&retry, Collector::Off),
    );
    let first = first.unwrap();
    let second = second.unwrap();
    first.close().await.unwrap();
    second.close().await.unwrap();
    drop(held);
}

/// A store in a temporary folder whose command is cancelled closes before its folder is removed,
/// and where the drop cannot wait the next command in this process does the two steps: no folder is
/// left behind, with a database still open inside it.
#[tokio::test]
async fn a_cancelled_temp_store_leaves_no_folder() {
    let dir = tempfile::tempdir().unwrap();
    let temp = TempStore::open(dir.path(), Collector::Off).await.unwrap();
    let path = temp.path().to_path_buf();
    assert!(path.is_dir());
    // A cancelled command drops the value where nothing can be awaited.
    drop(temp);
    // The next command in this process opens a store, which is where a store and a folder that were
    // handed on are closed and then removed. This waits for those two steps, not for a time.
    let other = Store::open(&dir.path().join("other"), Collector::Off)
        .await
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while path.exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "a cancelled store left its folder behind"
        );
        tokio::task::yield_now().await;
    }
    other.close().await.unwrap();
}
