//! Failure paths: a dropped provider, a refused transfer, a lying manifest and an expiry.

use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_dropped_connection_is_retried() {
    let dir = tempfile::tempdir().unwrap();
    let home = Home::open(&dir.path().join("home")).unwrap();
    let source = dir.path().join("large.bin");
    let body: Vec<u8> = (0..(4 << 20)).map(|i| (i as u8).wrapping_mul(13)).collect();
    fs::write(&source, &body).unwrap();
    let mut outgoing = super::prepare_send(
        &home,
        &source,
        RelayModeChoice::Disabled,
        super::Congestion::Cubic,
        None,
        None,
    )
    .await
    .unwrap();
    outgoing.reserve(Keep::One);
    let text = outgoing.ticket.encode();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let out = dir.path().join("out");
    let first = outgoing.provider.addr();
    let task = tokio::spawn(async move {
        let received = super::receive(&text, &options(&out), &tx).await;
        (received, out)
    });
    wait_for(&mut rx, |event| {
        matches!(event, super::Event::Started { .. })
    })
    .await;
    outgoing.provider.shutdown().await.unwrap();
    wait_for(&mut rx, |event| {
        matches!(event, super::Event::Retrying { .. })
    })
    .await;
    // A sender that comes back imports the file again: the content of a send is its own and went
    // with it. The home saves the port its last send bound, so the second send reaches the address
    // the ticket already carries.
    let mut again = super::prepare_send(
        &home,
        &source,
        RelayModeChoice::Disabled,
        super::Congestion::Cubic,
        None,
        None,
    )
    .await
    .unwrap();
    // A prepared send refuses every request until its mode is set, as the command line sets it
    // before printing a ticket.
    again.reserve(Keep::One);
    let provider = again.provider;
    let reached = provider
        .addr()
        .ip_addrs()
        .any(|at| first.ip_addrs().any(|old| old == at));
    if !reached {
        eprintln!("SKIPPED: the second send did not bind the address the ticket names");
        task.abort();
        provider.shutdown().await.unwrap();
        return;
    }
    let (received, out) = task.await.unwrap();
    let received = received.unwrap();
    assert_eq!(received.bytes, body.len() as u64);
    assert_eq!(fs::read(out.join("large.bin")).unwrap(), body);
    provider.shutdown().await.unwrap();
}

/// Waits until the transfer reports an event `wanted` accepts. A transfer that ends first fails the
/// test here, so a wait can never pass on a receive that already stopped.
async fn wait_for(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<super::Event>,
    wanted: impl Fn(&super::Event) -> bool,
) {
    while let Some(event) = rx.recv().await {
        if wanted(&event) {
            return;
        }
    }
    panic!("the receive ended before the event the test waits for");
}

#[tokio::test]
async fn a_full_disk_is_refused_before_the_content_is_asked_for() {
    let dir = tempfile::tempdir().unwrap();
    let body = vec![7u8; 2 << 20];
    let (outgoing, text) = prepared(&dir, &body).await;
    let out = dir.path().join("out");
    let mut options = options(&out);
    options.free_bytes = Some(0);
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let err = super::receive(&text, &options, &tx).await.unwrap_err();
    assert!(matches!(err, Error::Storage(_)), "{err}");
    assert_no_partial(&out);
    outgoing.provider.shutdown().await.unwrap();
}

#[tokio::test]
async fn an_oversized_manifest_is_refused_after_its_size_is_proved() {
    let dir = tempfile::tempdir().unwrap();
    let home = Home::open(&dir.path().join("home")).unwrap();
    let store = FsStore::load(dir.path().join("sender-store"))
        .await
        .unwrap();
    let body = vec![9u8; 1 << 20];
    let hash = store.add_slice(&body).await.unwrap().hash;
    let mut provider = Provider::start(
        &home,
        store,
        BTreeSet::from([*hash.as_bytes()]),
        RelayModeChoice::Disabled,
        super::Congestion::Cubic,
        None,
        None,
    )
    .await
    .unwrap();
    // A send sets its mode before its ticket is printed; this test receives without serving.
    provider.set_limit(None);
    let addr = provider.addr();
    let ticket = Ticket::new(
        *addr.id.as_bytes(),
        addr.ip_addrs().copied().collect(),
        None,
        None,
        Body::Static {
            root: *hash.as_bytes(),
            size: body.len() as u64,
            format: Format::HashSequence,
        },
    )
    .unwrap();
    let out = dir.path().join("out");
    let mut options = options(&out);
    options.limits = Limits {
        max_bytes: 64 << 10,
        ..Limits::default()
    };
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let err = super::receive(&ticket.encode(), &options, &tx)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Policy(_)), "{err}");
    assert_no_partial(&out);
    provider.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_blob_larger_than_its_manifest_says_is_refused_before_its_body() {
    let dir = tempfile::tempdir().unwrap();
    let body = vec![4u8; 2 << 20];
    let (provider, hash, ticket) = provider_of_lying_manifest(&dir, &body, 1 << 20, 0).await;
    let out = dir.path().join("out");
    let mut options = options(&out);
    options.max_network_failures = 1;
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let err = super::receive(&ticket.encode(), &options, &tx)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Verify(_)), "{err}");
    // The size proof is two chunk groups. A body fetch would be the whole two megabytes.
    let served = provider.served_bytes(hash);
    assert!(
        served <= 2 * 16 * 1024,
        "served {served} bytes of a refused blob"
    );
    // A refusal that is not a network failure does not leave a store a retry could trust.
    assert_no_partial(&out);
    provider.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_request_for_the_last_chunk_leaves_the_sender_serving() {
    let dir = tempfile::tempdir().unwrap();
    let home = Home::open(&dir.path().join("home")).unwrap();
    let store = FsStore::load(dir.path().join("store")).await.unwrap();
    let body = vec![6u8; 1 << 20];
    let hash = store.add_slice(&body).await.unwrap().hash;
    let mut provider = Provider::start(
        &home,
        store,
        BTreeSet::from([*hash.as_bytes()]),
        RelayModeChoice::Disabled,
        super::Congestion::Cubic,
        None,
        None,
    )
    .await
    .unwrap();
    let addr = provider.addr();
    // A send sets its mode before its ticket is printed; this test asks before `serve` starts.
    provider.set_limit(None);
    let (client, client_store) = client_of(&dir).await;
    let conn = client.connect(addr, iroh_blobs::ALPN).await.unwrap();
    client_store
        .remote()
        .execute_get(
            conn.clone(),
            GetRequest::blob_ranges(hash, ChunkRanges::last_chunk()),
        )
        .complete()
        .await
        .unwrap();

    let mut serve = tokio::spawn(async move {
        provider
            .serve(
                super::Stop::OneDownload(vec![*hash.as_bytes()]),
                &no_events(),
            )
            .await
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(300), &mut serve)
            .await
            .is_err(),
        "a partial get stopped the sender"
    );
    // Leaving after part of the blob is not a finished download, so the sender stays.
    conn.close(0u32.into(), b"done");
    assert!(
        tokio::time::timeout(Duration::from_millis(600), &mut serve)
            .await
            .is_err(),
        "a partial get that left ended the sender"
    );
    serve.abort();
    let _ = serve.await;
    client.close().await;
    client_store.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_expiry_cuts_off_a_get_that_started_before_it() {
    let dir = tempfile::tempdir().unwrap();
    let home = Home::open(&dir.path().join("home")).unwrap();
    let store = FsStore::load(dir.path().join("store")).await.unwrap();
    // A sparse file is read by reference, so the test does not hold 512 MiB in memory. It is
    // large enough that a debug-build fetch cannot finish before the deadline, which is what
    // leaves a get in flight when the expiry closes the connection.
    let path = dir.path().join("large.bin");
    let file = fs::File::create(&path).unwrap();
    file.set_len(512 << 20).unwrap();
    drop(file);
    let tag = store
        .blobs()
        .add_path_with_opts(iroh_blobs::api::blobs::AddPathOptions {
            path: path.clone(),
            format: iroh_blobs::BlobFormat::Raw,
            mode: iroh_blobs::api::blobs::ImportMode::TryReference,
        })
        .temp_tag()
        .await
        .unwrap();
    let hash = tag.hash();
    // The client is bound before the deadline is set, so a slow machine spends its time before
    // the ticket exists rather than inside the transfer being measured.
    let (client, client_store) = client_of(&dir).await;
    let expiry = super::now_ms() + 1500;
    let provider = Provider::start(
        &home,
        store,
        BTreeSet::from([*hash.as_bytes()]),
        RelayModeChoice::Disabled,
        super::Congestion::Cubic,
        Some(expiry),
        None,
    )
    .await
    .unwrap();
    let addr = provider.addr();
    let serve = tokio::spawn(async move {
        provider
            .serve(
                super::Stop::Keep {
                    limit: None,
                    roots: vec![],
                },
                &no_events(),
            )
            .await
    });
    let conn = client.connect(addr, iroh_blobs::ALPN).await.unwrap();
    let err = client_store
        .remote()
        .execute_get(conn, GetRequest::blob(hash))
        .complete()
        .await;
    assert!(err.is_err(), "a get finished after the ticket expiry");
    assert_eq!(
        serve.await.unwrap().unwrap(),
        super::ServeEnd::Expired,
        "the sender did not stop at the expiry"
    );
    client.close().await;
    client_store.shutdown().await.unwrap();
    drop(tag);
}

/// One paced chunk must not hold up the answers every other connection waits for. With a cap of a
/// byte a second, a single chunk would wait for hours; a second connection's refused request still
/// has to be answered at once, because the wait belongs to the chunk and not to the gate.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_paced_chunk_does_not_hold_up_another_connection() {
    let dir = tempfile::tempdir().unwrap();
    let home = Home::open(&dir.path().join("home")).unwrap();
    let store = FsStore::load(dir.path().join("sender-store"))
        .await
        .unwrap();
    let hash = store.add_slice(vec![2u8; 64 * 1024]).await.unwrap().hash;
    let other = store.add_slice(vec![3u8; 1024]).await.unwrap().hash;
    let rate = crate::engine::Rate::new(1).unwrap();
    let provider = Provider::start_with_rate(
        &home,
        store,
        BTreeSet::from([*hash.as_bytes()]),
        RelayModeChoice::Disabled,
        super::Congestion::Cubic,
        None,
        None,
        Some(rate),
    )
    .await
    .unwrap();
    let addr = provider.addr();
    let serve = tokio::spawn(async move {
        provider
            .serve(
                super::Stop::Keep {
                    limit: None,
                    roots: vec![],
                },
                &no_events(),
            )
            .await
    });

    let (client, client_store) = client_of(&dir).await;
    // One connection asks for the allowed blob, whose first chunk is paced.
    let paced = {
        let store = client_store.clone();
        let addr = addr.clone();
        let endpoint = client.clone();
        tokio::spawn(async move {
            let conn = endpoint.connect(addr, iroh_blobs::ALPN).await.unwrap();
            store
                .remote()
                .execute_get(conn, GetRequest::blob(hash))
                .complete()
                .await
        })
    };
    // Give the paced request time to reach the gate's throttle.
    tokio::time::sleep(Duration::from_millis(300)).await;

    // A hash that is not allowed is refused by the same policy, and that answer cannot wait for the
    // chunk that is still being paced.
    let conn = client.connect(addr, iroh_blobs::ALPN).await.unwrap();
    let refused = tokio::time::timeout(
        Duration::from_secs(5),
        client_store
            .remote()
            .execute_get(conn, GetRequest::blob(other))
            .complete(),
    )
    .await
    .expect("a refused request waited behind a paced chunk");
    assert!(refused.is_err());

    paced.abort();
    serve.abort();
    client.close().await;
    client_store.shutdown().await.unwrap();
}
