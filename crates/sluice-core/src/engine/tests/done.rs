//! How a sender learns that a receive is finished: the receiver's completion code, and the byte
//! counting that stands in for a receiver that never sends one.

use super::*;

/// A client endpoint and a store, for a test that fetches part of a transfer by hand.
async fn client(dir: &TempDir) -> (Endpoint, FsStore) {
    let endpoint = endpoint::bind(None, &RelayModeChoice::Disabled, Congestion::Cubic, None)
        .await
        .unwrap();
    let store = FsStore::load(dir.path().join("client-store"))
        .await
        .unwrap();
    (endpoint, store)
}

/// The content root of a one-file transfer, which is what a client asks for.
fn content_root(outgoing: &crate::engine::Outgoing, text: &str) -> Hash {
    let ticket = Ticket::decode(text).unwrap();
    assert!(matches!(ticket.body(), Body::Static { .. }));
    Hash::from(outgoing.roots[0])
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_completion_code_ends_a_sender_that_was_asked_for_every_root() {
    let dir = tempfile::tempdir().unwrap();
    let body = vec![3u8; 64 << 10];
    let (outgoing, _text) = prepared(&dir, &body).await;
    let root = Hash::from(outgoing.roots[0]);
    let roots = outgoing.roots.clone();
    let addr = outgoing.provider.addr();
    let serve = tokio::spawn(async move {
        outgoing
            .provider
            .serve(Stop::OneDownload(roots), &no_events())
            .await
    });

    // A client that holds the ticket reads the whole content blob and then says the transfer is
    // complete. That is the signal a sender waits for on a resumed transfer, where its own byte
    // count never reaches the whole file.
    let (probe, store) = client(&dir).await;
    let conn = probe.connect(addr, iroh_blobs::ALPN).await.unwrap();
    crate::engine::fetch::fetch_whole(&store, &conn, root)
        .await
        .unwrap();
    conn.close(crate::engine::COMPLETE_CLOSE.into(), b"complete");

    let end = tokio::time::timeout(Duration::from_secs(5), serve)
        .await
        .expect("the completion code did not end the sender")
        .unwrap()
        .unwrap();
    assert_eq!(end, ServeEnd::Downloaded);
    probe.close().await;
}

/// The close code is a bare number on an unauthenticated connection, so a peer that never held a
/// ticket could otherwise end a send and make it print success. It counts only for a connection
/// that asked for every root of the transfer.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_peer_without_the_ticket_cannot_end_the_sender() {
    let dir = tempfile::tempdir().unwrap();
    let body = vec![4u8; 64 << 10];
    let (outgoing, text) = prepared(&dir, &body).await;
    let roots = outgoing.roots.clone();
    let addr = outgoing.provider.addr();
    let mut serve = tokio::spawn(async move {
        outgoing
            .provider
            .serve(Stop::OneDownload(roots), &no_events())
            .await
    });

    let (probe, _store) = client(&dir).await;
    let conn = probe.connect(addr, iroh_blobs::ALPN).await.unwrap();
    conn.close(crate::engine::COMPLETE_CLOSE.into(), b"complete");

    assert!(
        tokio::time::timeout(Duration::from_millis(2_500), &mut serve)
            .await
            .is_err(),
        "a peer that asked for nothing ended the sender"
    );

    // The sender is still the one serving this ticket, and the receive that follows ends it.
    let out = dir.path().join("out");
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    super::receive(&text, &options(&out), &tx).await.unwrap();
    let end = tokio::time::timeout(Duration::from_secs(10), serve)
        .await
        .expect("the sender did not stop after the transfer")
        .unwrap()
        .unwrap();
    assert_eq!(end, ServeEnd::Downloaded);
    probe.close().await;
}

/// Asking for every root is what makes a connection's completion code count, and the record
/// belongs to that connection. A later connection that asks for nothing cannot use it: iroh's id
/// for the closed connection is handed on to the next one, and a sender that believed the code of
/// the id rather than the connection would stop for a peer that received nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_later_connection_cannot_use_the_record_of_the_one_before_it() {
    let dir = tempfile::tempdir().unwrap();
    let body = vec![7u8; 64 << 10];
    let (outgoing, text) = prepared(&dir, &body).await;
    let root = Hash::from(outgoing.roots[0]);
    let roots = outgoing.roots.clone();
    let addr = outgoing.provider.addr();
    let mut serve = tokio::spawn(async move {
        outgoing
            .provider
            .serve(Stop::OneDownload(roots), &no_events())
            .await
    });

    // One connection asks for a byte of the root, which is recorded, and goes away without the
    // code, as a killed receive does.
    let (probe, _store) = client(&dir).await;
    let conn = probe.connect(addr.clone(), iroh_blobs::ALPN).await.unwrap();
    let store = MemStore::new();
    store
        .remote()
        .execute_get(
            conn.clone(),
            GetRequest::blob_ranges(root, ChunkRanges::bytes(0..1024)),
        )
        .complete()
        .await
        .unwrap();
    conn.close(0u32.into(), b"stopped");
    conn.closed().await;

    // A second connection asks for nothing and says the transfer is complete.
    let conn = probe.connect(addr.clone(), iroh_blobs::ALPN).await.unwrap();
    conn.close(crate::engine::COMPLETE_CLOSE.into(), b"complete");

    assert!(
        tokio::time::timeout(Duration::from_millis(2_500), &mut serve)
            .await
            .is_err(),
        "the record of a closed connection ended the sender for another one"
    );

    let out = dir.path().join("out");
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    super::receive(&text, &options(&out), &tx).await.unwrap();
    let end = tokio::time::timeout(Duration::from_secs(10), serve)
        .await
        .expect("the sender did not stop after the transfer")
        .unwrap()
        .unwrap();
    assert_eq!(end, ServeEnd::Downloaded);
    probe.close().await;
}

/// A peer that asked for one root of a two-root transfer has not received the transfer, so its
/// completion code is ignored. Asking one connection for both roots and saying so is what ends it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_peer_that_asked_for_one_root_of_two_cannot_end_the_sender() {
    let dir = tempfile::tempdir().unwrap();
    let home = Home::open(&dir.path().join("home")).unwrap();
    let store = FsStore::load(dir.path().join("store")).await.unwrap();
    let one = store.add_slice([5u8; 32 << 10]).await.unwrap().hash;
    let two = store.add_slice([6u8; 32 << 10]).await.unwrap().hash;
    let allowed: BTreeSet<[u8; 32]> = [*one.as_bytes(), *two.as_bytes()].into_iter().collect();
    let provider = Provider::start(
        &home,
        store,
        allowed,
        RelayModeChoice::Disabled,
        Congestion::Cubic,
        None,
        None,
    )
    .await
    .unwrap();
    let addr = provider.addr();
    let roots = vec![*one.as_bytes(), *two.as_bytes()];
    let mut serve =
        tokio::spawn(async move { provider.serve(Stop::OneDownload(roots), &no_events()).await });

    let (probe, store) = client(&dir).await;
    let conn = probe.connect(addr.clone(), iroh_blobs::ALPN).await.unwrap();
    crate::engine::fetch::fetch_whole(&store, &conn, one)
        .await
        .unwrap();
    conn.close(crate::engine::COMPLETE_CLOSE.into(), b"complete");

    assert!(
        tokio::time::timeout(Duration::from_millis(2_500), &mut serve)
            .await
            .is_err(),
        "one root of two ended the sender"
    );

    // One connection that asked for both roots is what the code counts for.
    let conn = probe.connect(addr, iroh_blobs::ALPN).await.unwrap();
    for root in [one, two] {
        crate::engine::fetch::fetch_whole(&store, &conn, root)
            .await
            .unwrap();
    }
    conn.close(crate::engine::COMPLETE_CLOSE.into(), b"complete");
    let end = tokio::time::timeout(Duration::from_secs(10), serve)
        .await
        .expect("asking for both roots did not end the sender")
        .unwrap()
        .unwrap();
    assert_eq!(end, ServeEnd::Downloaded);
    probe.close().await;
}

/// Two requests for the first half move the whole file's worth of bytes, and the sender still has
/// not delivered it. A sender that stopped here would leave a receiver whose second run asks for
/// the rest with nothing to fetch.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_same_half_twice_does_not_count_as_the_whole_file() {
    let dir = tempfile::tempdir().unwrap();
    let body: Vec<u8> = (0..128 * 1024).map(|i| i as u8).collect();
    let (outgoing, text) = prepared(&dir, &body).await;
    let hash = content_root(&outgoing, &text);
    let roots = outgoing.roots.clone();
    let addr = outgoing.provider.addr();
    let mut serve = tokio::spawn(async move {
        outgoing
            .provider
            .serve(Stop::OneDownload(roots), &no_events())
            .await
    });

    let (probe, _store) = client(&dir).await;
    for _ in 0..2 {
        let conn = probe.connect(addr.clone(), iroh_blobs::ALPN).await.unwrap();
        let store = MemStore::new();
        store
            .remote()
            .execute_get(
                conn.clone(),
                GetRequest::blob_ranges(hash, ChunkRanges::bytes(0..65_536)),
            )
            .complete()
            .await
            .unwrap();
        conn.close(0u32.into(), b"done");
    }

    // Longer than the grace that follows a delivered root, so a sender that counted the repeat
    // would already have stopped.
    assert!(
        tokio::time::timeout(Duration::from_millis(2_500), &mut serve)
            .await
            .is_err(),
        "the same half twice ended the sender"
    );
    serve.abort();
    probe.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_range_receive_leaves_the_sender_serving() {
    let dir = tempfile::tempdir().unwrap();
    let body: Vec<u8> = (0..256 * 1024).map(|i| (i % 251) as u8).collect();
    let (outgoing, text) = prepared(&dir, &body).await;
    let roots = outgoing.roots.clone();
    let mut serve = tokio::spawn(async move {
        outgoing
            .provider
            .serve(Stop::OneDownload(roots), &no_events())
            .await
    });

    let out = dir.path().join("out.bin");
    let store_root = dir.path().join("tmp");
    fs::create_dir_all(&store_root).unwrap();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let range = crate::range::ByteRange::parse("0:1000").unwrap();
    let bytes = crate::engine::receive_range(&text, range, &options(&out), &store_root, &tx)
        .await
        .unwrap();
    assert_eq!(bytes, 1000);
    assert_eq!(fs::read(out.join("source.bin")).unwrap().len(), 1000);

    assert!(
        tokio::time::timeout(Duration::from_millis(2_500), &mut serve)
            .await
            .is_err(),
        "a range receive ended the sender"
    );
    serve.abort();
}

/// A receive that was cut off and then finished against a new sender: the new sender served only
/// the bytes the first one was missing, so its own byte count never reaches the whole file. The
/// completion code is what ends it, and the file is what proves the resume worked.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_receive_that_resumes_against_a_new_sender_ends_that_sender() {
    let dir = tempfile::tempdir().unwrap();
    let body: Vec<u8> = (0..(4 << 20)).map(|i| (i % 251) as u8).collect();
    let (first, text) = prepared(&dir, &body).await;
    let out = dir.path().join("out");
    let mut cut = options(&out);
    cut.max_network_failures = 1;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let text_for_task = text.clone();
    let options_for_task = cut.clone();
    let first_tx = tx.clone();
    let task =
        tokio::spawn(
            async move { super::receive(&text_for_task, &options_for_task, &first_tx).await },
        );
    // Wait until content is moving, then take the sender away the way a killed process does.
    while let Some(event) = rx.recv().await {
        if matches!(event, Event::Progress { bytes, .. } if bytes > 0) {
            break;
        }
    }
    first.provider.shutdown().await.unwrap();
    let err = task.await.unwrap().unwrap_err();
    assert!(matches!(err, Error::Network(_)), "{err}");

    let (second, text) = prepared(&dir, &body).await;
    let roots = second.roots.clone();
    let serve = tokio::spawn(async move {
        second
            .provider
            .serve(Stop::OneDownload(roots), &no_events())
            .await
    });
    super::receive(&text, &options(&out), &tx).await.unwrap();
    assert_eq!(fs::read(out.join("source.bin")).unwrap(), body);

    let end = tokio::time::timeout(Duration::from_secs(10), serve)
        .await
        .expect("the resumed receive did not end the new sender")
        .unwrap()
        .unwrap();
    assert_eq!(end, ServeEnd::Downloaded);
}
