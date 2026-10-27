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
            congestion: super::Congestion::Cubic,
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
