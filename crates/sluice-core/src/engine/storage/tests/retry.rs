//! The paths a removal of a command keeps, and what the end of that command does with them: a
//! removal it could not finish is tried again when the command ends, a path a claim has made its
//! own is left, and a partial store whose removal gave up is noted so the next claim of the output
//! removes it.
/// A removal queued on a runtime that is shut down before the task starts is kept, not lost: a
/// blocking task the runtime drops takes a plain count with it, so the path is recorded as left and
/// the end of the command removes the folder itself.
#[test]
fn a_removal_that_never_ran_is_kept_for_the_end_of_the_command() {
    use std::time::Duration;

    use super::super::{removals::Queued, wait_for_removals_at};

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
    // The folder this test made is the only one it asks about: no home is read, and none is made.
    wait_for_removals_at(Duration::from_secs(2), None);
    assert!(!folder.exists(), "the kept folder was left for the sweep");
}

/// A removal that was already kept is tried once more even when the budget is gone: a command whose
/// own removals were still parked when it began to end used the whole wait, and the folder would
/// otherwise sit until the sweep, which removes one only a day old.
#[test]
fn a_path_kept_before_the_budget_ran_out_is_still_tried() {
    use std::time::Duration;

    use super::super::{removals::left, wait_for_removals_at};

    let _alone = crate::engine::storage::exclusive_removals();
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("folder");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(folder.join("file"), b"bytes").unwrap();
    // What a removal that gave up leaves, and what the retry is for.
    left(&folder);

    wait_for_removals_at(Duration::ZERO, None);
    assert!(
        !folder.exists(),
        "a kept folder was left for the sweep because the budget had gone"
    );
}

/// The paths a removal keeps are keyed by the one form the hold uses, so a later removal that
/// spells the folder another way clears the entry: nothing retries a folder that is gone or that a
/// claim now owns.
#[test]
fn a_kept_path_is_the_one_the_hold_is_keyed_by() {
    use std::time::Duration;

    use super::super::{
        removals::{gone, left},
        wait_for_removals_at,
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
    wait_for_removals_at(Duration::from_secs(1), None);
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
        wait_for_removals_at,
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
    wait_for_removals_at(Duration::from_secs(1), None);
    assert!(
        *seen.lock().unwrap(),
        "a path kept during a retry pass was dropped by it"
    );
}

/// A path a removal cannot take yet is tried on every tick until the budget is gone, and then once
/// more: a pass that begins after the deadline makes one attempt and ends the wait, so a folder
/// that never goes cannot keep a command alive, and a folder that goes on the last moment still
/// goes.
#[test]
fn one_pass_after_the_budget_ends_the_wait() {
    use std::{
        sync::{Arc, Mutex},
        time::{Duration, Instant},
    };

    use super::super::{
        removals::{left, seam},
        wait_for_removals_at,
    };

    let _alone = crate::engine::storage::exclusive_removals();
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("folder");
    std::fs::create_dir(&folder).unwrap();
    left(&folder);
    // A hold another thread keeps is what makes the removal wait: the folder itself could always be
    // removed, and a pass that blocks on the hold is what the budget would then be spent on.
    let (holding, held) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel();
    let at = folder.clone();
    let worker = std::thread::spawn(move || {
        super::super::registry::with_hold(&at, || {
            holding.send(()).unwrap();
            released.recv().unwrap()
        })
    });
    held.recv().unwrap();

    let seen = Arc::new(Mutex::new(Vec::new()));
    let passes = Arc::clone(&seen);
    let _watched = seam::watch(move |spent| passes.lock().unwrap().push(spent));
    let budget = Duration::from_millis(200);
    let began = Instant::now();
    wait_for_removals_at(budget, None);
    let elapsed = began.elapsed();
    release.send(()).unwrap();
    worker.join().unwrap().unwrap();

    let seen = seen.lock().unwrap().clone();
    assert!(seen.len() > 1, "the wait made {} passes", seen.len());
    assert_eq!(
        seen.iter().filter(|spent| **spent).count(),
        1,
        "the passes were {seen:?}"
    );
    assert!(
        elapsed >= budget,
        "the wait returned after {elapsed:?} of a {budget:?} budget"
    );
}

/// The end of a command removes a kept partial store that no record names: it is a folder a removal
/// of this command left, and the names are read to tell it from a store a claim has made since.
#[test]
fn the_end_of_a_command_removes_a_kept_partial_store_no_record_names() {
    use std::time::Duration;

    use super::super::{removals::left, wait_for_removals_at};

    let _alone = crate::engine::storage::exclusive_removals();
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out");
    std::fs::create_dir_all(&out).unwrap();
    let store = out.join(".sluice-partial-x");
    std::fs::create_dir(&store).unwrap();
    std::fs::write(store.join("file"), b"bytes").unwrap();
    left(&store);

    wait_for_removals_at(Duration::from_secs(1), None);
    assert!(
        !store.exists(),
        "a partial store no record names was left for the sweep"
    );
}

/// A partial store whose removal gave up is noted, and the next claim of that output removes it
/// rather than refusing a name no record explains. The note is made through the record the store
/// had and stands for the folder alone, so it cannot outlive it.
#[test]
fn a_partial_store_a_removal_gave_up_on_is_noted_for_the_next_claim() {
    use super::super::{discard_here, discarded, forget_record, remember_record};

    let _alone = crate::engine::storage::exclusive_removals();
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out");
    std::fs::create_dir_all(&out).unwrap();
    let store = out.join(".sluice-partial-x");
    std::fs::create_dir(&store).unwrap();
    std::fs::write(store.join("file"), b"bytes").unwrap();
    remember_record(&store, "root-one").unwrap();

    let held = Held::in_folder(&store);
    discard_here(&store);
    assert!(store.exists(), "the removal was expected to give up");
    forget_record(&store);
    assert!(
        discarded(&store),
        "a folder a removal gave up on is not noted"
    );

    drop(held);
    // The next claim of the output asks `discarded`, removes the folder and starts its own store.
    assert!(
        discarded(&store),
        "a note does not survive until it is honoured"
    );
    discard_here(&store);
    assert!(!store.exists(), "the noted folder was left for the user");
    assert!(!discarded(&store), "the note outlived its folder");
}

/// A folder a removal of a test cannot take, released when it is dropped: no write permission on
/// the folder itself on Unix, and a file in it that another handle does not share on Windows.
struct Held {
    #[cfg(unix)]
    folder: std::path::PathBuf,
    #[cfg(windows)]
    _file: std::fs::File,
}

impl Held {
    #[cfg(unix)]
    fn in_folder(folder: &std::path::Path) -> Self {
        use std::os::unix::fs::PermissionsExt;

        let mut mode = std::fs::metadata(folder).unwrap().permissions();
        mode.set_mode(0o500);
        std::fs::set_permissions(folder, mode).unwrap();
        Self {
            folder: folder.to_path_buf(),
        }
    }

    #[cfg(windows)]
    fn in_folder(folder: &std::path::Path) -> Self {
        use std::os::windows::fs::OpenOptionsExt;

        let path = folder.join("held");
        std::fs::write(&path, b"held").unwrap();
        let file = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&path)
            .unwrap();
        Self { _file: file }
    }
}

impl Drop for Held {
    #[cfg(unix)]
    fn drop(&mut self) {
        use std::os::unix::fs::PermissionsExt;

        let mut mode = std::fs::metadata(&self.folder).unwrap().permissions();
        mode.set_mode(0o700);
        let _ = std::fs::set_permissions(&self.folder, mode);
    }

    #[cfg(windows)]
    fn drop(&mut self) {}
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
