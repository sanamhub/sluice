//! The rules that refuse or replace a target, and the metadata a file keeps.

use super::*;

#[tokio::test]
async fn one_clash_refuses_the_whole_folder_before_content() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("tree");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("a.txt"), b"new").unwrap();
    fs::write(source.join("b.txt"), b"new b").unwrap();
    let (outgoing, text) = prepared_paths(&dir, &[source]).await;
    let out = dir.path().join("out");
    fs::create_dir_all(out.join("tree")).unwrap();
    fs::write(out.join("tree").join("a.txt"), b"old").unwrap();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let err = super::receive(&text, &options(&out), &tx)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Policy(_)), "{err}");
    assert!(err.to_string().contains("--force"), "{err}");
    assert_eq!(fs::read(out.join("tree").join("a.txt")).unwrap(), b"old");
    assert!(!out.join("tree").join("b.txt").exists());
    assert_no_partial(&out);
    outgoing.provider.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn force_writes_into_a_folder_that_is_already_there() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("tree");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("a.txt"), b"new").unwrap();
    let (outgoing, text) = prepared_paths(&dir, &[source]).await;
    let out = dir.path().join("out");
    fs::create_dir_all(out.join("tree")).unwrap();
    fs::write(out.join("tree").join("a.txt"), b"old").unwrap();
    let mut options = options(&out);
    options.force = true;
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    super::receive(&text, &options, &tx).await.unwrap();
    assert_eq!(fs::read(out.join("tree").join("a.txt")).unwrap(), b"new");
    outgoing.provider.shutdown().await.unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn links_are_created_only_when_asked() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("tree");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("a.txt"), b"the target").unwrap();
    std::os::unix::fs::symlink("a.txt", source.join("link")).unwrap();

    let (outgoing, text) = prepared_paths(&dir, std::slice::from_ref(&source)).await;
    let out = dir.path().join("without");
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    super::receive(&text, &options(&out), &tx).await.unwrap();
    assert!(fs::symlink_metadata(out.join("tree").join("link")).is_err());
    outgoing.provider.shutdown().await.unwrap();

    let (outgoing, text) = prepared_paths(&dir, &[source]).await;
    let out = dir.path().join("with");
    let mut options = options(&out);
    options.links = true;
    super::receive(&text, &options, &tx).await.unwrap();
    let link = out.join("tree").join("link");
    assert!(
        fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(fs::read(&link).unwrap(), b"the target");
    outgoing.provider.shutdown().await.unwrap();
}

#[tokio::test]
async fn the_time_is_restored_unless_told_not_to() {
    let dir = tempfile::tempdir().unwrap();
    let home = Home::open(&dir.path().join("home")).unwrap();
    let source = dir.path().join("source.bin");
    let body = b"a file with a known time";
    fs::write(&source, body).unwrap();
    let at = UNIX_EPOCH + Duration::from_secs(1_600_000_000);
    set_mtime(&source, at);
    let wanted = mtime_ms(&source);
    let outgoing = super::prepare_send(
        &home,
        &source,
        RelayModeChoice::Disabled,
        super::Congestion::Cubic,
        None,
        None,
    )
    .await
    .unwrap();
    let text = outgoing.ticket.encode();
    let out = dir.path().join("out");
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let received = super::receive(&text, &options(&out), &tx).await.unwrap();
    assert_eq!(mtime_ms(&received.path), wanted);
    outgoing.provider.shutdown().await.unwrap();

    let outgoing = super::prepare_send(
        &home,
        &source,
        RelayModeChoice::Disabled,
        super::Congestion::Cubic,
        None,
        None,
    )
    .await
    .unwrap();
    let text = outgoing.ticket.encode();
    let out = dir.path().join("untimed");
    let mut untimed = options(&out);
    untimed.no_times = true;
    let received = super::receive(&text, &untimed, &tx).await.unwrap();
    assert_ne!(mtime_ms(&received.path), wanted);
    outgoing.provider.shutdown().await.unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn mode_bits_are_applied_without_setuid_or_setgid() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let home = Home::open(&dir.path().join("home")).unwrap();
    let source = dir.path().join("source.bin");
    let body = b"an executable";
    fs::write(&source, body).unwrap();
    fs::set_permissions(&source, fs::Permissions::from_mode(0o6755)).unwrap();
    assert_eq!(
        fs::metadata(&source).unwrap().permissions().mode() & 0o6000,
        0o6000
    );
    let outgoing = super::prepare_send(
        &home,
        &source,
        RelayModeChoice::Disabled,
        super::Congestion::Cubic,
        None,
        None,
    )
    .await
    .unwrap();
    let text = outgoing.ticket.encode();
    let (serve_tx, _serve_rx) = tokio::sync::mpsc::unbounded_channel();
    let serve = tokio::spawn(async move { outgoing.serve(false, &serve_tx).await });
    let out = dir.path().join("out");
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let received = super::receive(&text, &options(&out), &tx).await.unwrap();
    let mode = fs::metadata(&received.path).unwrap().permissions().mode() & 0o7777;
    assert_eq!(mode, 0o755);
    serve.await.unwrap().unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn force_replaces_a_symlink_and_leaves_its_target() {
    use std::os::unix::fs::symlink;

    let dir = tempfile::tempdir().unwrap();
    let (outgoing, text) = prepared(&dir, b"the new body").await;
    let out = dir.path().join("out");
    fs::create_dir(&out).unwrap();
    let target = out.join("target");
    fs::write(&target, b"keep me").unwrap();
    symlink("target", out.join("source.bin")).unwrap();
    let mut options = options(&out);
    options.force = true;
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let received = super::receive(&text, &options, &tx).await.unwrap();
    assert!(fs::symlink_metadata(&received.path).unwrap().is_file());
    assert_eq!(fs::read(&target).unwrap(), b"keep me");
    assert_eq!(fs::read(&received.path).unwrap(), b"the new body");
    outgoing.provider.shutdown().await.unwrap();
}
#[tokio::test]
async fn a_hidden_top_level_name_is_refused_until_allowed() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join(".env");
    fs::write(&source, b"SECRET=1").unwrap();
    let (outgoing, text) = prepared_paths(&dir, std::slice::from_ref(&source)).await;
    let out = dir.path().join("out");
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let err = super::receive(&text, &options(&out), &tx)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Policy(_)), "{err}");
    assert!(err.to_string().contains("--allow-hidden"), "{err}");
    assert!(!out.join(".env").exists());
    outgoing.provider.shutdown().await.unwrap();

    let (outgoing, text) = prepared_paths(&dir, &[source]).await;
    let mut options = options(&out);
    options.allow_hidden = true;
    super::receive(&text, &options, &tx).await.unwrap();
    assert_eq!(fs::read(out.join(".env")).unwrap(), b"SECRET=1");
    outgoing.provider.shutdown().await.unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn a_link_in_the_output_cannot_carry_a_write_outside() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("tree");
    fs::create_dir_all(source.join("d")).unwrap();
    fs::write(source.join("d").join("x"), b"new").unwrap();
    let (outgoing, text) = prepared_paths(&dir, &[source]).await;
    let out = dir.path().join("out");
    let outside = dir.path().join("outside");
    fs::create_dir_all(&out).unwrap();
    fs::create_dir(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, out.join("tree")).unwrap();
    let mut options = options(&out);
    options.force = true;
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let err = super::receive(&text, &options, &tx).await.unwrap_err();
    assert!(matches!(err, Error::Policy(_)), "{err}");
    assert!(!outside.join("d").join("x").exists());
    outgoing.provider.shutdown().await.unwrap();
}

/// A link may only reach a `..` after a component that is a link of the same transfer, and that
/// link may move the path up. Reading the target as text alone would accept both of these.
#[cfg(unix)]
#[tokio::test]
async fn a_link_chain_that_escapes_the_output_is_refused() {
    for (names, targets) in [
        (("a", "z"), ("..", "a/..")),
        // The same chain with the escaping link sorting before the one it reads through.
        (("z", "m"), ("..", "z/..")),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("sub");
        fs::create_dir(&source).unwrap();
        std::os::unix::fs::symlink(targets.0, source.join(names.0)).unwrap();
        std::os::unix::fs::symlink(targets.1, source.join(names.1)).unwrap();
        let (outgoing, text) = prepared_paths(&dir, std::slice::from_ref(&source)).await;
        let out = dir.path().join("out");
        let mut options = options(&out);
        options.links = true;
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let err = super::receive(&text, &options, &tx).await.unwrap_err();
        assert!(matches!(err, Error::Policy(_)), "{err}");
        assert!(!out.join("sub").join(names.0).exists());
        assert!(!out.join("sub").join(names.1).exists());
        outgoing.provider.shutdown().await.unwrap();
    }
}

/// A folder named like a partial store belongs to a receive only when this home has a record of it.
/// One that does not is refused and left exactly as it was: it could have been planted where the
/// store was, and resuming from it would trust bytes nobody in this home wrote.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_partial_folder_this_home_did_not_record_is_refused_and_kept() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("tree");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("a.txt"), b"same").unwrap();
    let (outgoing, text) = prepared_paths(&dir, std::slice::from_ref(&source)).await;
    let ticket = Ticket::decode(&text).unwrap();
    let Body::Static { root, .. } = ticket.body() else {
        unreachable!("a folder ticket is static")
    };
    let owner: String = root.iter().map(|byte| format!("{byte:02x}")).collect();
    let out = dir.path().join("out");
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();

    // A folder with a marker that names another ticket, which this home never recorded.
    let partial = crate::engine::storage::partial_dir(&out, &owner);
    fs::create_dir_all(&partial).unwrap();
    fs::write(
        partial.join(crate::engine::storage::MARKER),
        "another ticket\n",
    )
    .unwrap();
    fs::write(partial.join("junk"), b"planted").unwrap();
    let err = super::receive(&text, &options(&out), &tx)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Policy(_)), "{err}");
    assert!(err.to_string().contains(".sluice-partial-"), "{err}");
    assert!(
        partial.join("junk").exists(),
        "the planted folder was changed"
    );

    // This home owns a store here now: the record and the marker id make it a resume. A file the
    // writer finds changed is still a clash, and a clash removes both the store and its record.
    crate::engine::storage::claim(&partial).unwrap();
    let id = crate::engine::storage::remember_record(&partial, &owner).unwrap();
    let names = std::collections::BTreeSet::from(["tree".to_string()]);
    crate::engine::storage::remember(&partial, &owner, &names, false, &id, "all").unwrap();
    fs::create_dir_all(out.join("tree")).unwrap();
    fs::write(out.join("tree").join("a.txt"), b"clobbered").unwrap();
    let err = super::receive(&text, &options(&out), &tx)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Policy(_)), "{err}");
    assert!(!partial.exists(), "a clash at write time kept the store");
    outgoing.provider.shutdown().await.unwrap();
}

/// A sender never puts a name in a manifest that the path rules reserve for a receive's own store:
/// a folder or file called `.sluice-tmp-*` or `.sluice-partial-*` is refused when the send starts.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_source_that_names_a_reserved_folder_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let home = Home::open(&dir.path().join("home")).unwrap();
    let source = dir.path().join("tree");
    fs::create_dir(&source).unwrap();
    fs::write(source.join(".sluice-tmp-0123456789abcdef"), b"x").unwrap();
    let sent = crate::engine::prepare_send_paths(
        &home,
        std::slice::from_ref(&source),
        RelayModeChoice::Disabled,
        Congestion::Cubic,
        None,
        None,
    )
    .await;
    let err = match sent {
        Ok(_) => panic!("a reserved name was sent"),
        Err(e) => e,
    };
    assert!(matches!(err, Error::Policy(_)), "{err}");
    assert!(err.to_string().contains("reject"), "{err}");
}
