//! What a receive writes: one file, a folder, and several paths at once.

use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn one_file_goes_out_and_arrives_with_its_name() {
    let dir = tempfile::tempdir().unwrap();
    let home = Home::open(&dir.path().join("home")).unwrap();
    let source = dir.path().join("holiday photo.jpg");
    let body = b"a tiny file that fits in one pack";
    std::fs::write(&source, body).unwrap();

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

    let out = dir.path().join("received");
    let (recv_tx, _recv_rx) = tokio::sync::mpsc::unbounded_channel();
    let received = super::receive(
        &text,
        &super::ReceiveOptions {
            out: out.clone(),
            force: false,
            no_times: false,
            links: false,
            allow_hidden: false,
            limits: crate::manifest::Limits::default(),
            free_bytes: None,
            max_size: None,
            congestion: super::Congestion::Cubic,
            rate: None,
            write_limit: None,
            max_network_failures: crate::queue::MAX_NETWORK_FAILURES,
        },
        &recv_tx,
    )
    .await
    .unwrap();
    assert_eq!(received.bytes, body.len() as u64);
    assert_eq!(received.path, out.join("holiday photo.jpg"));
    assert_eq!(std::fs::read(&received.path).unwrap(), body);
    let end = serve.await.unwrap().unwrap();
    assert_eq!(end, super::StopEnd::Downloaded);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_folder_arrives_with_packs_and_empty_folders() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("photos");
    fs::create_dir_all(source.join("sub")).unwrap();
    fs::create_dir(source.join("empty")).unwrap();
    let big: Vec<u8> = (0..(2 << 20)).map(|i| (i as u8).wrapping_mul(17)).collect();
    fs::write(source.join("big.bin"), &big).unwrap();
    fs::write(source.join("small.txt"), b"a small file").unwrap();
    fs::write(source.join("sub").join("note.txt"), b"nested").unwrap();

    let (outgoing, text) = prepared_paths(&dir, &[source]).await;
    let (serve_tx, _serve_rx) = tokio::sync::mpsc::unbounded_channel();
    let serve = tokio::spawn(async move { outgoing.serve(false, &serve_tx).await });
    let out = dir.path().join("received");
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let received = super::receive(&text, &options(&out), &tx).await.unwrap();
    assert_eq!(received.files, 3);
    assert_eq!(fs::read(out.join("photos").join("big.bin")).unwrap(), big);
    assert_eq!(
        fs::read(out.join("photos").join("small.txt")).unwrap(),
        b"a small file"
    );
    assert_eq!(
        fs::read(out.join("photos").join("sub").join("note.txt")).unwrap(),
        b"nested"
    );
    assert!(out.join("photos").join("empty").is_dir());
    assert_eq!(serve.await.unwrap().unwrap(), super::StopEnd::Downloaded);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn several_paths_keep_their_names() {
    let dir = tempfile::tempdir().unwrap();
    let one = dir.path().join("a.mov");
    let folder = dir.path().join("photos");
    fs::write(&one, b"one file").unwrap();
    fs::create_dir(&folder).unwrap();
    fs::write(folder.join("b.txt"), b"in a folder").unwrap();

    let (outgoing, text) = prepared_paths(&dir, &[one, folder]).await;
    let (serve_tx, _serve_rx) = tokio::sync::mpsc::unbounded_channel();
    let serve = tokio::spawn(async move { outgoing.serve(false, &serve_tx).await });
    let out = dir.path().join("received");
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let received = super::receive(&text, &options(&out), &tx).await.unwrap();
    assert_eq!(received.files, 2);
    assert_eq!(fs::read(out.join("a.mov")).unwrap(), b"one file");
    assert_eq!(
        fs::read(out.join("photos").join("b.txt")).unwrap(),
        b"in a folder"
    );
    assert_eq!(serve.await.unwrap().unwrap(), super::StopEnd::Downloaded);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_empty_folder_crosses_and_stops_the_sender() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("empty");
    fs::create_dir(&source).unwrap();
    let (outgoing, text) = prepared_paths(&dir, &[source]).await;
    let (serve_tx, _serve_rx) = tokio::sync::mpsc::unbounded_channel();
    let serve = tokio::spawn(async move { outgoing.serve(false, &serve_tx).await });
    let out = dir.path().join("received");
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let received = super::receive(&text, &options(&out), &tx).await.unwrap();
    assert_eq!(received.files, 0);
    assert!(out.is_dir());
    assert_eq!(serve.await.unwrap().unwrap(), super::StopEnd::Downloaded);
}

/// A static send copies a small file into its own store, because a pack cannot be referenced where
/// it sits, and keeps the manifest and the hash sequence there too. The store is a folder of its
/// own, and when the send has served its one download the folder is gone: the home holds none of
/// the send's bytes, and the `blobs` folder an older home imported into is not there at all.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_static_send_leaves_none_of_its_bytes_in_the_home() {
    let dir = tempfile::tempdir().unwrap();
    let home = Home::open(&dir.path().join("home")).unwrap();
    let source = dir.path().join("small.bin");
    std::fs::write(&source, vec![5u8; 4096]).unwrap();
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
    let scratch = outgoing
        .scratch()
        .expect("a send has a store")
        .to_path_buf();
    assert_eq!(
        scratch.parent(),
        Some(home.temporary_folder().unwrap().as_path()),
        "the store is not in the home's temporary area"
    );
    let text = outgoing.ticket.encode();
    let (serve_tx, _serve_rx) = tokio::sync::mpsc::unbounded_channel();
    let serve = tokio::spawn(async move { outgoing.serve(false, &serve_tx).await });
    let out = dir.path().join("received");
    let (recv_tx, _recv_rx) = tokio::sync::mpsc::unbounded_channel();
    let received = super::receive(&text, &options(&out), &recv_tx)
        .await
        .unwrap();
    assert_eq!(received.bytes, 4096);
    assert_eq!(serve.await.unwrap().unwrap(), StopEnd::Downloaded);
    assert!(!scratch.exists(), "the send kept its store folder");
    assert_eq!(send_bytes(&home), 0, "the send left bytes in the home");
    assert!(
        !home.path().join("blobs").exists(),
        "the home still has a blobs folder"
    );
}

/// Standard input is copied into the send's own store, because the pipe is gone by the time the
/// ticket exists. When the send has served its one download the folder goes, and so does the copy:
/// the home holds none of those bytes and no `blobs` folder.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stdin_send_leaves_none_of_its_bytes_in_the_home() {
    let dir = tempfile::tempdir().unwrap();
    let home = Home::open(&dir.path().join("home")).unwrap();
    let body = vec![7u8; 256 * 1024];
    let outgoing = crate::engine::prepare_send_stdin_from(&home, body.clone())
        .await
        .unwrap();
    let scratch = outgoing
        .scratch()
        .expect("a send has a store")
        .to_path_buf();
    let text = outgoing.ticket.encode();
    let root = match outgoing.ticket.body() {
        Body::Static { root, .. } => *root,
        Body::Live { .. } => panic!("a live ticket"),
    };
    let (serve_tx, _serve_rx) = tokio::sync::mpsc::unbounded_channel();
    let serve = tokio::spawn(async move { outgoing.serve(false, &serve_tx).await });

    // A receiver takes the whole raw blob, which is the download the send waits for.
    let receiver = Receiver::open(RelayModeChoice::Disabled, Congestion::Cubic)
        .await
        .unwrap();
    let (recv_tx, _recv_rx) = tokio::sync::mpsc::unbounded_channel();
    let ticket = crate::ticket::Ticket::decode(&text).unwrap();
    let conn = receiver.connect(&ticket, recv_tx).await.unwrap();
    let store = FsStore::load(dir.path().join("receiver-store"))
        .await
        .unwrap();
    crate::engine::fetch::fetch_whole(&store, &conn, Hash::from(root))
        .await
        .unwrap();
    let got = crate::engine::fetch::complete_bytes(&store, Hash::from(root))
        .await
        .unwrap();
    assert_eq!(got.0, body);
    assert_eq!(serve.await.unwrap().unwrap(), StopEnd::Downloaded);
    assert!(!scratch.exists(), "the send kept its store folder");
    assert_eq!(send_bytes(&home), 0, "the send left bytes in the home");
    assert!(
        !home.path().join("blobs").exists(),
        "the home still has a blobs folder"
    );
}

/// Two sends in one home each open a store of their own and neither removes the other's: both
/// copies arrive, both sends finish, and each removes only its own folder.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_sends_in_one_home_both_finish() {
    let dir = tempfile::tempdir().unwrap();
    let home = Home::open(&dir.path().join("home")).unwrap();
    let first = dir.path().join("first.bin");
    let second = dir.path().join("second.bin");
    std::fs::write(&first, vec![1u8; 96 * 1024]).unwrap();
    std::fs::write(&second, vec![2u8; 128 * 1024]).unwrap();
    // Both stores are open at the same time, which is what two sends in one home look like.
    let one = super::prepare_send(
        &home,
        &first,
        RelayModeChoice::Disabled,
        super::Congestion::Cubic,
        None,
        None,
    )
    .await
    .unwrap();
    let two = super::prepare_send(
        &home,
        &second,
        RelayModeChoice::Disabled,
        super::Congestion::Cubic,
        None,
        None,
    )
    .await
    .unwrap();
    let one_dir = one.scratch().expect("a send has a store").to_path_buf();
    let two_dir = two.scratch().expect("a send has a store").to_path_buf();
    assert_ne!(one_dir, two_dir, "two sends shared one store");
    let one_text = one.ticket.encode();
    let two_text = two.ticket.encode();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let tx_one = tx.clone();
    let serving_one = tokio::spawn(async move { one.serve(false, &tx_one).await });
    let serving_two = tokio::spawn(async move { two.serve(false, &tx).await });

    let (recv_tx, _recv_rx) = tokio::sync::mpsc::unbounded_channel();
    let one_options = options(&dir.path().join("out-one"));
    let two_options = options(&dir.path().join("out-two"));
    let (got_one, got_two) = tokio::join!(
        super::receive(&one_text, &one_options, &recv_tx),
        super::receive(&two_text, &two_options, &recv_tx),
    );
    assert_eq!(got_one.unwrap().bytes, 96 * 1024);
    assert_eq!(got_two.unwrap().bytes, 128 * 1024);
    assert_eq!(serving_one.await.unwrap().unwrap(), StopEnd::Downloaded);
    assert_eq!(serving_two.await.unwrap().unwrap(), StopEnd::Downloaded);
    assert!(
        !one_dir.exists() && !two_dir.exists(),
        "a store folder was kept"
    );
    assert_eq!(send_bytes(&home), 0, "a send left bytes in the home");
}

/// A send that fails after its store is open removes the store with it: the provider could not
/// start, so nothing was served, and the home holds neither the copies nor the folder.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_send_whose_provider_cannot_start_removes_its_store() {
    let dir = tempfile::tempdir().unwrap();
    let home = Home::open(&dir.path().join("home")).unwrap();
    let source = dir.path().join("small.bin");
    std::fs::write(&source, vec![3u8; 4096]).unwrap();
    // The provider reads the home's identity when it binds its endpoint, and a key that is a folder
    // is refused there, after this send has already put its bytes in its own store.
    std::fs::create_dir(home.path().join("identity.key")).unwrap();
    let Err(err) = super::prepare_send(
        &home,
        &source,
        RelayModeChoice::Disabled,
        super::Congestion::Cubic,
        None,
        None,
    )
    .await
    else {
        panic!("a send with an identity that is a folder served");
    };
    assert!(!matches!(err, Error::Network(_)), "{err}");
    assert_eq!(
        send_bytes(&home),
        0,
        "the failed send left bytes in the home"
    );
    assert!(
        !home.path().join("blobs").exists(),
        "the failed send left a blobs folder"
    );
}

/// A publish that is dropped before it serves has nothing left to await a close with. Its store is
/// handed to the next command, which closes it, and the folder goes after it: a cancelled send
/// leaves no store and no bytes in the home.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_publish_dropped_without_serving_leaves_no_store_in_the_home() {
    let dir = tempfile::tempdir().unwrap();
    let home = Home::open(&dir.path().join("home")).unwrap();
    let source = dir.path().join("small.bin");
    std::fs::write(&source, vec![9u8; 4096]).unwrap();
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
    let scratch = outgoing
        .scratch()
        .expect("a send has a store")
        .to_path_buf();
    assert!(scratch.is_dir(), "the send has no store folder");
    // Dropping the value is what a cancelled command does: no future is left to await a close.
    drop(outgoing);

    // The next command in this process opens a store, which is where a store that was handed on is
    // closed; the folder goes after that, so this waits for the two steps and not for a time.
    let other = crate::engine::storage::Store::open(
        &dir.path().join("settle-store"),
        crate::engine::storage::Collector::Off,
    )
    .await
    .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while scratch.exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "the cancelled send kept its store folder"
        );
        tokio::task::yield_now().await;
    }
    other.close().await.unwrap();
    assert_eq!(send_bytes(&home), 0, "the cancelled send left bytes");
}
