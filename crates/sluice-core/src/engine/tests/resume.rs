//! What a resume may write into: the names the first run made, the flag that run needed, and the
//! mode of a folder the run itself made.

use super::*;

/// A resumed receive writes into a folder the user already had, and leaves its mode and time alone:
/// it belongs to the output now, not to this transfer. The first run had to be forced to write
/// there, so the resume is forced too.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_resume_does_not_take_over_a_folder_that_was_already_there() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("tree");
    fs::create_dir_all(source.join("sub")).unwrap();
    fs::write(source.join("big.bin"), vec![7u8; 900 * 1024]).unwrap();
    fs::write(source.join("sub").join("x.txt"), b"x").unwrap();
    let (outgoing, text) = prepared_paths(&dir, std::slice::from_ref(&source)).await;
    let out = dir.path().join("out");
    // The user's own folder, which only `--force` may write into.
    fs::create_dir_all(out.join("tree")).unwrap();
    let mut limited = options(&out);
    limited.force = true;
    limited.write_limit = Some(256 * 1024);
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    super::receive(&text, &limited, &tx).await.unwrap_err();
    assert!(out.join("tree").join("sub").is_dir());

    // What the user did to the folder between the two runs. Windows has no folder mode to set and
    // does not restore folder times, so there the resume only has to leave the tree alone.
    let tree = out.join("tree");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&tree, fs::Permissions::from_mode(0o750)).unwrap();
    }

    let mut resumed = options(&out);
    resumed.force = true;
    super::receive(&text, &resumed, &tx).await.unwrap();
    assert_eq!(fs::read(tree.join("big.bin")).unwrap().len(), 900 * 1024);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // The mode is the user's, and it is not what a receive that took the folder over
        // leaves. Writing a child moves the folder's time on every platform, so the time is
        // not asserted here; it is set by the same branch as the mode.
        assert_eq!(
            fs::metadata(&tree).unwrap().permissions().mode() & 0o777,
            0o750
        );
    }
    outgoing.provider.shutdown().await.unwrap();
}

/// A folder the cut-off run made is this transfer's, so the resume that finds it already there
/// still restores its mode. Leaving it out would leave every folder a cut-off run made with the
/// mode a plain `create_dir` gives.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_resume_restores_the_mode_of_a_folder_its_own_run_made() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("tree");
    fs::create_dir_all(source.join("sub")).unwrap();
    fs::write(source.join("big.bin"), vec![7u8; 900 * 1024]).unwrap();
    fs::write(source.join("sub").join("x.txt"), b"x").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // A mode the sender records and a plain `create_dir` never sets.
        fs::set_permissions(&source, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let (outgoing, text) = prepared_paths(&dir, std::slice::from_ref(&source)).await;
    let out = dir.path().join("out");
    let mut limited = options(&out);
    limited.write_limit = Some(256 * 1024);
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    super::receive(&text, &limited, &tx).await.unwrap_err();
    let tree = out.join("tree");
    assert!(tree.is_dir());

    super::receive(&text, &options(&out), &tx).await.unwrap();
    assert_eq!(fs::read(tree.join("big.bin")).unwrap().len(), 900 * 1024);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&tree).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
    outgoing.provider.shutdown().await.unwrap();
}

/// A first run that was allowed to write into a folder the user already had cannot hand that
/// folder to a later run: the marker names no top-level entry, because the run created none, and the
/// marker says the run was forced. A resume without `--force` is refused instead of writing new
/// files into the user's folder.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_forced_run_is_not_resumed_without_the_flag() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("tree");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("big.bin"), vec![7u8; 900 * 1024]).unwrap();
    let (outgoing, text) = prepared_paths(&dir, std::slice::from_ref(&source)).await;
    let out = dir.path().join("out");
    // The user's own folder of that name, which only `--force` may write into.
    fs::create_dir_all(out.join("tree")).unwrap();
    fs::write(out.join("tree").join("notes.txt"), b"mine").unwrap();

    let mut forced = options(&out);
    forced.force = true;
    forced.write_limit = Some(256 * 1024);
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let err = super::receive(&text, &forced, &tx).await.unwrap_err();
    assert!(matches!(err, Error::Storage(_)), "{err}");
    assert!(partial_exists(&out), "the failed run removed its store");

    let err = super::receive(&text, &options(&out), &tx)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Policy(_)), "{err}");
    assert!(err.to_string().contains("--force"), "{err}");
    assert_eq!(
        fs::read(out.join("tree").join("notes.txt")).unwrap(),
        b"mine"
    );

    // The run that means it says so again, and finishes into the folder it took over.
    let mut again = options(&out);
    again.force = true;
    super::receive(&text, &again, &tx).await.unwrap();
    assert_eq!(
        fs::read(out.join("tree").join("big.bin")).unwrap().len(),
        900 * 1024
    );
    outgoing.provider.shutdown().await.unwrap();
}

/// A store has to be this transfer's, not merely this home's: a record whose id a marker carries but
/// whose root is another ticket's names a store another receive is building. Refusing it leaves the
/// folder, its marker and the record exactly as they were, and starts no new store beside them.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_store_of_another_transfer_is_refused_without_removing_it() {
    let dir = tempfile::tempdir().unwrap();

    let one = dir.path().join("one.bin");
    fs::write(&one, vec![1u8; 64 * 1024]).unwrap();
    let two = dir.path().join("two.bin");
    fs::write(&two, vec![2u8; 64 * 1024]).unwrap();

    let (outgoing, text_one) =
        prepared_paths_in(&dir, "home-one", std::slice::from_ref(&one)).await;

    let (second, text_two) = prepared_paths_in(&dir, "home-two", std::slice::from_ref(&two)).await;

    let owners: Vec<String> = [&text_one, &text_two]
        .iter()
        .map(|text| {
            let ticket = Ticket::decode(text).unwrap();
            let Body::Static { root, .. } = ticket.body() else {
                unreachable!("a file ticket is static")
            };
            root.iter().map(|byte| format!("{byte:02x}")).collect()
        })
        .collect();

    let out = dir.path().join("out");
    fs::create_dir_all(&out).unwrap();

    let partial = crate::engine::storage::partial_dir(&out, &owners[1]);
    crate::engine::storage::claim(&partial).unwrap();

    let names = std::collections::BTreeSet::new();
    // The marker of the first transfer's store, under the name the second transfer would use, and a
    // record for the second transfer that carries the same id.
    crate::engine::storage::remember(&partial, &owners[0], &names, false, "a-record", "all")
        .unwrap();
    crate::engine::storage::remember_as(&partial, &owners[1], "a-record").unwrap();
    let marker = fs::read_to_string(partial.join(crate::engine::storage::MARKER)).unwrap();

    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let err = super::receive(&text_two, &options(&out), &tx)
        .await
        .unwrap_err();

    assert!(matches!(err, Error::Policy(_)), "{err}");
    assert!(err.to_string().contains(".sluice-partial-"), "{err}");
    assert!(
        partial.is_dir(),
        "the store of another transfer was removed"
    );
    assert_eq!(
        fs::read_to_string(partial.join(crate::engine::storage::MARKER)).unwrap(),
        marker
    );
    assert!(!out.join("two.bin").exists());

    // This transfer still has its own store to make beside that one.
    super::receive(&text_one, &options(&out), &tx)
        .await
        .unwrap();
    assert_eq!(fs::read(out.join("one.bin")).unwrap().len(), 64 * 1024);
    outgoing.provider.shutdown().await.unwrap();
    second.provider.shutdown().await.unwrap();
}

/// A receive that creates one top-level name and writes into another that was already there records
/// only the one it made, so a resume without `--force` may write into its own folder and not into
/// the user's.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_resume_writes_only_into_the_names_the_first_run_made() {
    let dir = tempfile::tempdir().unwrap();
    let mine = dir.path().join("tree");
    fs::create_dir(&mine).unwrap();
    fs::write(mine.join("big.bin"), vec![7u8; 900 * 1024]).unwrap();
    let loose = dir.path().join("loose.bin");
    fs::write(&loose, vec![8u8; 64 * 1024]).unwrap();
    let (outgoing, text) = prepared_paths(&dir, &[mine.clone(), loose.clone()]).await;
    let out = dir.path().join("out");
    // One of the two top-level names is already the user's, and only `--force` may write into it;
    // the other one is new, and the run creates it.
    fs::create_dir_all(out.join("tree")).unwrap();

    let mut forced = options(&out);
    forced.force = true;
    forced.write_limit = Some(256 * 1024);
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    super::receive(&text, &forced, &tx).await.unwrap_err();
    assert!(partial_exists(&out), "the failed run removed its store");

    // The marker names the folder this run made and not the one it found, and it says the run was
    // forced so a later run cannot continue it quietly.
    let marker = fs::read_dir(&out)
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .find(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with(".sluice-partial-"))
        })
        .map(|path| fs::read_to_string(path.join("ticket")).unwrap())
        .unwrap();
    assert!(marker.contains("name loose.bin"), "{marker}");
    assert!(!marker.contains("name tree"), "{marker}");
    assert!(
        marker.lines().any(|line| line.trim() == "force"),
        "{marker}"
    );

    let mut again = options(&out);
    again.force = true;
    super::receive(&text, &again, &tx).await.unwrap();
    assert_eq!(
        fs::read(out.join("tree").join("big.bin")).unwrap().len(),
        900 * 1024
    );
    assert_eq!(fs::read(out.join("loose.bin")).unwrap().len(), 64 * 1024);
    outgoing.provider.shutdown().await.unwrap();
}

/// A store kept for a resume does not collect, and this shows it by watching the bytes across sweeps
/// that are known to have run rather than by waiting for one. A second store opened with the
/// collector on holds one blob after another whose tag is dropped; each one that is gone is a sweep
/// that happened, and a store that collected would have freed the resume store's bytes at one of
/// them. The run that continues the store then finds the blobs it needs.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_kept_store_keeps_its_blobs_across_a_sweep() {
    let dir = tempfile::tempdir().unwrap();
    // Distinct bytes, so the blob is one this store had to fetch rather than one it made up.
    let mut body = vec![0u8; 4 << 20];
    for (at, byte) in body.iter_mut().enumerate() {
        *byte = (at % 251) as u8;
    }
    let (outgoing, text) = prepared(&dir, &body).await;
    let out = dir.path().join("out");
    let mut slow = options(&out);
    slow.rate = Some(crate::engine::Rate::new(256 * 1024).unwrap());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let first = {
        let text = text.clone();
        let tx = tx.clone();
        tokio::spawn(async move { super::receive(&text, &slow, &tx).await })
    };
    let database = wait_for_open_store(&out).await;
    let store = database.parent().unwrap().to_path_buf();
    // The store holds what the first run fetched, and this waits for the fetch to have written it.
    let held = wait_for_bytes(&store).await;
    // The cancelled run drops its store where nothing can be awaited, so its tags are gone with it:
    // only the folder holds the bytes now, which is the state a sweep would take them from.
    first.abort();
    assert!(first.await.unwrap_err().is_cancelled());

    // Ten sweeps of a control store, each awaited as the event it is. If the resume store collected,
    // one of them would have freed its bytes, and the check after every one would see it.
    let control = crate::engine::storage::open_store_after(
        &dir.path().join("control-store"),
        Duration::from_millis(20),
    )
    .await
    .unwrap();
    for _ in 0..10 {
        let tag = control
            .blobs()
            .add_slice(vec![3u8; 128 * 1024])
            .temp_tag()
            .await
            .unwrap();
        let hash = tag.hash();
        drop(tag);
        wait_until_gone(&control, hash).await;
        let now = bytes_in(&store);
        assert!(
            now >= held,
            "a sweep took {} of the {held} bytes the resume store kept",
            held.saturating_sub(now)
        );
    }
    control.shutdown().await.unwrap();

    let received = super::receive(&text, &options(&out), &tx).await.unwrap();
    assert_eq!(received.bytes, body.len() as u64);
    assert_eq!(fs::read(out.join("source.bin")).unwrap(), body);
    outgoing.provider.shutdown().await.unwrap();
}

/// How many bytes the data files of the store at `store` hold.
fn bytes_in(store: &Path) -> u64 {
    fs::read_dir(store.join("data"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| entry.metadata().ok())
        .map(|meta| meta.len())
        .sum()
}

/// Waits until the store at `store` holds at least a megabyte, and returns what it held then.
async fn wait_for_bytes(store: &Path) -> u64 {
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    loop {
        let bytes = bytes_in(store);
        if bytes >= 1 << 20 {
            return bytes;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the receive never wrote bytes into its store"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Waits until the collector of `store` has freed `hash`, which is a sweep that happened.
async fn wait_until_gone(store: &FsStore, hash: iroh_blobs::Hash) {
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    loop {
        if !store.blobs().has(hash).await.unwrap() {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the control store's collector never swept"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// Waits until a receive has opened the store beside `out`, and returns its database path. The
/// folder exists a moment before the store does, so a test that cancels waits for the database:
/// from that moment the folder is locked, which is what a resume in the same process has to get
/// free again.
async fn wait_for_open_store(out: &Path) -> std::path::PathBuf {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        if let Ok(entries) = fs::read_dir(out) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                let database = entry.path().join("blobs.db");
                if name.starts_with(".sluice-partial-") && database.is_file() {
                    return database;
                }
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the receive never opened its store"
        );
        tokio::task::yield_now().await;
    }
}

/// A receive that is cancelled mid-transfer drops its future where nothing can be awaited. The
/// store it had open has to be closed on the way out, and the next command in this process closes
/// it before it opens anything, so the resume opens the same folder rather than finding its
/// database still locked.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cancelled_receive_is_resumed_in_the_same_process() {
    let dir = tempfile::tempdir().unwrap();
    // Eight megabytes of distinct bytes, so the transfer takes long enough to be cut off.
    let mut body = vec![0u8; 8 << 20];
    for (at, byte) in body.iter_mut().enumerate() {
        *byte = (at % 253) as u8;
    }
    let (outgoing, text) = prepared(&dir, &body).await;
    let out = dir.path().join("out");
    let (serve_tx, _serve_rx) = tokio::sync::mpsc::unbounded_channel();
    let serving = tokio::spawn(async move { outgoing.serve(false, &serve_tx).await });

    let mut slow = options(&out);
    slow.rate = Some(crate::engine::Rate::new(256 * 1024).unwrap());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let first = {
        let text = text.clone();
        let tx = tx.clone();
        tokio::spawn(async move { super::receive(&text, &slow, &tx).await })
    };
    let database = wait_for_open_store(&out).await;
    assert!(database.is_file());
    first.abort();
    assert!(first.await.unwrap_err().is_cancelled());

    let received = super::receive(&text, &options(&out), &tx).await.unwrap();
    assert_eq!(received.bytes, body.len() as u64);
    assert_eq!(fs::read(out.join("source.bin")).unwrap(), body);
    assert_eq!(serving.await.unwrap().unwrap(), StopEnd::Downloaded);
}
