//! Removing what a command made: one attempt, then a bounded wait for a file that is still held.

/// An open of another store does not wait for a temporary folder whose removal is still being
/// tried. The removal is parked on an event; the open finishes while it is parked, so the wait is
/// the store's close and not the folder's.
#[tokio::test]
#[allow(clippy::await_holding_lock)] // the one test lock, held for the whole test
async fn an_open_is_not_held_by_a_removal_that_is_retrying() {
    use std::time::Duration;

    use crate::engine::storage::{Collector, Store, TempStore};

    let _alone = crate::engine::storage::exclusive_removals();
    let dir = tempfile::tempdir().unwrap();
    let temp = TempStore::open(dir.path(), Collector::Off).await.unwrap();
    let folder = temp.path().to_path_buf();
    let (started, release) = super::super::parked::arm(&folder);
    // Dropping the pair hands the store and its folder to the next command, the way a cancelled
    // command does; the next open closes the store and then tries to remove the folder.
    drop(temp);
    tokio::task::spawn_blocking(move || started.recv().unwrap())
        .await
        .unwrap();

    let other = dir.path().join("other");
    std::fs::create_dir(&other).unwrap();
    // The removal is parked, so the folder is not gone. The open has to finish anyway: the lock it
    // waits on was released when the store closed. The timeout only keeps a regression from
    // hanging the test.
    let store = tokio::time::timeout(Duration::from_secs(5), Store::open(&other, Collector::Off))
        .await
        .expect("an open waited for a removal that was still being tried")
        .unwrap();
    store.close().await.unwrap();

    release.send(()).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while folder.exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "the temporary folder stayed"
        );
        tokio::task::yield_now().await;
    }
}

/// A removal a command left running is waited for when the command ends, so the folder is gone by
/// then. A blocking task is not joined by the end of the runtime, and a folder left behind stays
/// until the sweep, which only removes one that is a day old.
#[test]
fn the_end_of_a_command_waits_for_a_removal_that_is_still_running() {
    use std::time::Duration;

    use super::super::{parked, remove_quietly, wait_for_removals};

    let _alone = crate::engine::storage::exclusive_removals();
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("folder");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(folder.join("file"), b"bytes").unwrap();
    let (started, release) = parked::arm(&folder);
    // Nothing awaits this removal, the way a blocking task of `Temp::remove_apart` and the thread
    // of a dropped `Temp` are left.
    let at = folder.clone();
    let worker = std::thread::spawn(move || remove_quietly(&at));
    started.recv().unwrap();
    // The removal goes on only when this releases it, so a wait that returned at once would leave
    // the folder and fail the assertion below.
    let freed = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(100));
        release.send(()).unwrap();
    });
    wait_for_removals(Duration::from_secs(2));
    worker.join().unwrap();
    freed.join().unwrap();
    assert!(!folder.exists(), "the folder was left for the sweep");
}

/// A folder a hold kept is tried again when the command ends: a removal that gave up is not the
/// last word, so a command that ends leaves nothing it made. The hold is released once the first
/// removal has returned, so the retry has the file free whatever a slow machine did with the clock.
#[cfg(windows)]
#[test]
fn the_end_of_a_command_tries_a_removal_that_gave_up() {
    use std::{
        os::windows::fs::OpenOptionsExt,
        time::{Duration, Instant},
    };

    use super::super::{remove_quietly, wait_for_removals};

    let _alone = crate::engine::storage::exclusive_removals();
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("folder");
    std::fs::create_dir(&folder).unwrap();
    let file = folder.join("held");
    std::fs::write(&file, b"held").unwrap();
    let held = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&file)
        .unwrap();
    let (release, released) = std::sync::mpsc::channel();
    let holder = std::thread::spawn(move || {
        let _ = released.recv();
        drop(held);
    });
    remove_quietly(&folder);
    assert!(folder.exists(), "the removal was expected to give up");
    release.send(()).unwrap();
    holder.join().unwrap();
    let began = Instant::now();
    wait_for_removals(Duration::from_secs(2));
    assert!(!folder.exists(), "the folder was left for the sweep");
    assert!(
        began.elapsed() < Duration::from_secs(2),
        "the wait ran past its budget in {:?}",
        began.elapsed()
    );
}

/// A file that a scanner or a store's own close holds for a moment is released while the removal
/// keeps trying, and the folder still goes without the command seeing a failure.
#[cfg(windows)]
#[test]
fn a_folder_held_for_a_moment_is_still_removed() {
    use std::{
        os::windows::fs::OpenOptionsExt,
        time::{Duration, Instant},
    };

    let _alone = crate::engine::storage::exclusive_removals();
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("folder");
    std::fs::create_dir(&folder).unwrap();
    let file = folder.join("held");
    std::fs::write(&file, b"held").unwrap();
    let held = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&file)
        .unwrap();
    let released = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(80));
        drop(held);
    });
    let started = Instant::now();
    super::super::remove_quietly(&folder);
    released.join().unwrap();
    assert!(!folder.exists(), "the folder stayed");
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "the removal waited for {:?}",
        started.elapsed()
    );
}

/// A removal queued on a runtime that is shut down before the task starts is kept, not lost: a
/// blocking task the runtime drops takes a plain count with it, so the path is recorded as left and
/// the end of the command removes the folder itself.
#[test]
fn a_removal_that_never_ran_is_kept_for_the_end_of_the_command() {
    use std::time::Duration;

    use super::super::{removals::Queued, wait_for_removals};

    let _alone = crate::engine::storage::exclusive_removals();
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("folder");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(folder.join("file"), b"bytes").unwrap();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    // A runtime that has stopped accepts no blocking task: the task is dropped where it is spawned,
    // which is what happens to a task still queued when a command's runtime ends.
    let handle = runtime.handle().clone();
    runtime.shutdown_background();
    let queued = Queued::begin(folder.clone());
    drop(handle.spawn_blocking(move || queued.run()));

    assert!(folder.exists(), "the removal ran on a stopped runtime");
    wait_for_removals(Duration::from_secs(2));
    assert!(!folder.exists(), "the kept folder was left for the sweep");
}

/// The paths a removal keeps are keyed by the one form the hold uses, so a later removal that
/// spells the folder another way clears the entry: nothing retries a folder that is gone or that a
/// claim now owns.
#[test]
fn a_kept_path_is_the_one_the_hold_is_keyed_by() {
    use std::time::Duration;

    use super::super::{
        removals::{gone, left},
        wait_for_removals,
    };

    let _alone = crate::engine::storage::exclusive_removals();
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out");
    std::fs::create_dir_all(&out).unwrap();
    let store = out.join(".sluice-partial-x");
    std::fs::create_dir(&store).unwrap();
    std::fs::write(store.join("file"), b"bytes").unwrap();
    let spelled_differently = out.join("..").join("out").join(".sluice-partial-x");

    left(&spelled_differently);
    gone(&store);
    wait_for_removals(Duration::from_secs(1));
    assert!(
        store.exists(),
        "a kept path was retried under another spelling"
    );
}

/// The end of a command does not remove a kept path that a record names: a claim may have made it a
/// live store since the removal gave up, under another spelling of the same folder, and the folder
/// is that claim's.
#[test]
fn the_end_of_a_command_leaves_a_kept_store_a_record_names() {
    use std::time::Duration;

    use super::super::{forget_record, remember_record, removals::left, wait_for_removals};

    let _alone = crate::engine::storage::exclusive_removals();
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out");
    std::fs::create_dir_all(&out).unwrap();
    let store = out.join(".sluice-partial-x");
    std::fs::create_dir(&store).unwrap();
    std::fs::write(store.join("file"), b"bytes").unwrap();
    remember_record(&store, "root-one").unwrap();
    left(&out.join("..").join("out").join(".sluice-partial-x"));

    wait_for_removals(Duration::from_secs(1));
    assert!(
        store.exists(),
        "the end of the command removed a store a record names"
    );
    forget_record(&store);
}

/// A path another removal keeps while a retry pass runs is still in the list once the pass puts its
/// own survivors back: the pass appends to what is there instead of replacing it.
#[test]
fn a_path_kept_while_a_retry_runs_is_not_lost() {
    use std::{
        sync::{Arc, Mutex},
        time::Duration,
    };

    use super::super::{
        registry::canonical,
        removals::{kept_paths, left, seam},
        wait_for_removals,
    };

    let _alone = crate::engine::storage::exclusive_removals();
    let dir = tempfile::tempdir().unwrap();
    // A path that is gone gives the pass something to take and nothing to put back.
    left(&dir.path().join("gone"));
    // The path kept while the pass runs is the one that must survive it.
    let kept = dir.path().join("kept");
    std::fs::create_dir(&kept).unwrap();
    let wanted = canonical(&kept).unwrap();
    let seen = Arc::new(Mutex::new(false));
    let flag = Arc::clone(&seen);
    seam::arm(
        move || left(&kept),
        move || *flag.lock().unwrap() = kept_paths().contains(&wanted),
    );
    wait_for_removals(Duration::from_secs(1));
    assert!(
        *seen.lock().unwrap(),
        "a path kept during a retry pass was dropped by it"
    );
}

/// Only an error a moment can clear is worth another try: a name that is gone or a path that cannot
/// be removed at all is not, so the command is not delayed for it.
#[cfg(windows)]
#[test]
fn only_a_held_name_is_tried_again() {
    for code in [5, 32, 33, 145] {
        assert!(
            super::super::retryable(&std::io::Error::from_raw_os_error(code)),
            "{code}"
        );
    }
    for code in [2, 3, 267] {
        assert!(
            !super::super::retryable(&std::io::Error::from_raw_os_error(code)),
            "{code}"
        );
    }
}

#[cfg(unix)]
#[test]
fn only_a_busy_name_is_tried_again() {
    for code in [
        rustix::io::Errno::BUSY.raw_os_error(),
        rustix::io::Errno::NOTEMPTY.raw_os_error(),
    ] {
        assert!(
            super::super::retryable(&std::io::Error::from_raw_os_error(code)),
            "{code}"
        );
    }
    assert!(!super::super::retryable(
        &std::io::Error::from_raw_os_error(2)
    ));
}
