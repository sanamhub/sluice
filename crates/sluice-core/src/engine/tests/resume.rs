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
