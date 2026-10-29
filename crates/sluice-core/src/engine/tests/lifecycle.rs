//! What happens to the store and the sender at the end of a transfer: a kept store after a
//! network failure, a removed one after a write, and a sender that keeps serving a partial get.

use super::*;

fn partial_exists(out: &Path) -> bool {
    fs::read_dir(out).unwrap().flatten().any(|entry| {
        entry
            .file_name()
            .to_string_lossy()
            .starts_with(".sluice-partial-")
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_network_failure_keeps_the_partial_store_for_a_resume() {
    let dir = tempfile::tempdir().unwrap();
    let body = vec![5u8; 32 << 20];
    let (outgoing, text) = prepared(&dir, &body).await;
    let out = dir.path().join("out");
    let mut options = options(&out);
    options.max_network_failures = 1;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let task = tokio::spawn(async move { super::receive(&text, &options, &tx).await });
    while let Some(event) = rx.recv().await {
        if matches!(event, super::Event::Started { .. }) {
            break;
        }
    }
    outgoing.provider.shutdown().await.unwrap();
    let err = task.await.unwrap().unwrap_err();
    assert!(matches!(err, Error::Network(_)), "{err}");
    assert!(partial_exists(&out), "the partial store was removed");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_range_request_leaves_the_sender_serving() {
    let dir = tempfile::tempdir().unwrap();
    let home = Home::open(&dir.path().join("home")).unwrap();
    let store = FsStore::load(dir.path().join("store")).await.unwrap();
    let body = vec![7u8; 1 << 20];
    let hash = store.add_slice(&body).await.unwrap().hash;
    let provider = Provider::start(
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
    let (client, client_store) = client_of(&dir).await;
    let conn = client.connect(addr, iroh_blobs::ALPN).await.unwrap();
    crate::engine::fetch::fetch_range(
        &client_store,
        &conn,
        hash,
        ChunkRanges::bytes(1000..2000),
        None,
    )
    .await
    .unwrap();
    let mut serve = tokio::spawn(async move {
        provider
            .serve(super::Stop::OneDownload(vec![*hash.as_bytes()]))
            .await
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(300), &mut serve)
            .await
            .is_err(),
        "a byte range stopped the sender"
    );
    conn.close(0u32.into(), b"done");
    // Most of the blob was never served, so the client leaving is not a finished download and
    // the sender has to stay for a receive that comes back for the rest.
    assert!(
        tokio::time::timeout(Duration::from_millis(600), &mut serve)
            .await
            .is_err(),
        "a partial range that left ended the sender"
    );
    serve.abort();
    let _ = serve.await;
    client.close().await;
    client_store.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_time_the_platform_cannot_hold_is_skipped_with_a_warning() {
    let dir = tempfile::tempdir().unwrap();
    let body = b"a tiny body";
    let (provider, _hash, ticket) =
        provider_of_lying_manifest(&dir, body, body.len() as u64, u64::MAX).await;
    let out = dir.path().join("out");
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    super::receive(&ticket.encode(), &options(&out), &tx)
        .await
        .unwrap();
    assert_eq!(fs::read(out.join("big.bin")).unwrap(), body);
    let mut warned = false;
    while let Ok(event) = rx.try_recv() {
        if matches!(event, super::Event::Warning { .. }) {
            warned = true;
        }
    }
    // Windows cannot hold a time that far ahead and says so; Linux can, and keeps it. Either way
    // the file is written and nothing panics.
    #[cfg(windows)]
    assert!(warned, "the file was written without a warning");
    #[cfg(not(windows))]
    let _ = warned;
    provider.shutdown().await.unwrap();
}

/// A disk that fills while a file is being written leaves the store and a `Storage` error, and a
/// second receive with room finishes from that store. The write hook stands in for `ENOSPC`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_full_disk_at_write_time_keeps_the_store_for_the_next_run() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("tree");
    fs::create_dir(&source).unwrap();
    let body = vec![7u8; 900 * 1024];
    fs::write(source.join("big.bin"), &body).unwrap();
    let (outgoing, text) = prepared_paths(&dir, std::slice::from_ref(&source)).await;
    let out = dir.path().join("out");
    let mut limited = options(&out);
    limited.write_limit = Some(256 * 1024);
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let err = super::receive(&text, &limited, &tx).await.unwrap_err();
    assert!(matches!(err, Error::Storage(_)), "{err}");
    assert!(
        partial_exists(&out),
        "a full disk removed the store a retry needs"
    );
    assert!(!out.join("tree").join("big.bin").exists());

    super::receive(&text, &options(&out), &tx).await.unwrap();
    assert_eq!(fs::read(out.join("tree").join("big.bin")).unwrap(), body);
    outgoing.provider.shutdown().await.unwrap();
}

/// A provider that serves the hash sequence and refuses the manifest, so a receive fails between
/// the two: before any check, and before it may claim a place for its partial content.
async fn provider_of_sequence_only(
    dir: &TempDir,
) -> (Provider, FsStore, BTreeSet<[u8; 32]>, Ticket) {
    let home = Home::open(&dir.path().join("sequence-home")).unwrap();
    let store = FsStore::load(dir.path().join("sequence-store"))
        .await
        .unwrap();
    let manifest = Manifest::new(
        vec![Entry {
            path: ManifestPath::parse("tree").unwrap(),
            kind: Kind::Dir,
            size: 0,
            mtime_ms: 0,
            mode: 0,
            loc: Loc::None,
            link_target: None,
        }],
        vec![],
    )
    .unwrap();
    store.blobs().add_bytes(manifest.encode()).await.unwrap();
    store
        .blobs()
        .add_bytes(manifest.sequence_bytes())
        .await
        .unwrap();
    let allowed: BTreeSet<[u8; 32]> = BTreeSet::from([manifest.sequence_root()]);
    let provider = Provider::start(
        &home,
        store.clone(),
        allowed.clone(),
        RelayModeChoice::Disabled,
        super::Congestion::Cubic,
        None,
        None,
    )
    .await
    .unwrap();
    let addr = provider.addr();
    let ticket = Ticket::new(
        *addr.id.as_bytes(),
        addr.ip_addrs().copied().collect(),
        None,
        None,
        Body::Static {
            root: manifest.sequence_root(),
            size: 0,
            format: Format::HashSequence,
        },
    )
    .unwrap();
    let mut full = allowed;
    full.extend(manifest.hash_sequence());
    (provider, store, full, ticket)
}

/// A receive that fails before its checks leaves no store for the next run to resume.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failure_before_the_checks_leaves_no_store_to_resume() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out");
    let (first, _store, _allowed, ticket) = provider_of_sequence_only(&dir).await;
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let err = super::receive(&ticket.encode(), &options(&out), &tx)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Network(_)), "{err}");
    assert_no_partial(&out);
    first.shutdown().await.unwrap();
}

/// A client that reads the manifest and leaves has taken nothing the ticket promised, so the
/// sender has to stay for the transfer that follows.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_probe_only_client_leaves_the_sender_serving() {
    let dir = tempfile::tempdir().unwrap();
    let body = vec![4u8; 64 << 10];
    let (outgoing, text) = prepared(&dir, &body).await;
    let ticket = Ticket::decode(&text).unwrap();
    let Body::Static { root, .. } = ticket.body() else {
        unreachable!("a file ticket is static")
    };
    let sequence = Hash::from(*root);
    let probe_store = FsStore::load(dir.path().join("probe-store")).await.unwrap();
    let probe = endpoint::bind(
        None,
        &RelayModeChoice::Disabled,
        super::Congestion::Cubic,
        None,
    )
    .await
    .unwrap();
    let conn = probe
        .connect(outgoing.provider.addr(), iroh_blobs::ALPN)
        .await
        .unwrap();
    crate::engine::fetch::fetch_whole(&probe_store, &conn, sequence)
        .await
        .unwrap();
    conn.close(0u32.into(), b"probe");
    probe_store.shutdown().await.unwrap();
    probe.close().await;

    let roots = outgoing.roots.clone();
    let mut serve = tokio::spawn(async move {
        outgoing
            .provider
            .serve(super::Stop::OneDownload(roots))
            .await
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(300), &mut serve)
            .await
            .is_err(),
        "a probe that read only the manifest ended the sender"
    );
    let out = dir.path().join("out");
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    super::receive(&text, &options(&out), &tx).await.unwrap();
    let end = tokio::time::timeout(Duration::from_secs(5), serve)
        .await
        .expect("the sender did not stop after the real receive")
        .unwrap()
        .unwrap();
    assert_eq!(end, super::ServeEnd::Downloaded);
}

/// The grace after the last root is delivered is one deadline, not a timer every new connection
/// restarts: a client that reconnects once a second cannot keep a one-download sender alive.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_client_that_keeps_reconnecting_does_not_hold_the_sender_past_its_grace() {
    let dir = tempfile::tempdir().unwrap();
    let body = vec![6u8; 64 << 10];
    let (outgoing, text) = prepared(&dir, &body).await;
    let ticket = Ticket::decode(&text).unwrap();
    let Body::Static { root, .. } = ticket.body() else {
        unreachable!("a file ticket is static")
    };
    let sequence = Hash::from(*root);
    let addr = outgoing.provider.addr();
    let roots = outgoing.roots.clone();
    let serving = roots.clone();
    let serve = tokio::spawn(async move {
        outgoing
            .provider
            .serve(super::Stop::OneDownload(serving))
            .await
    });

    // One connection takes every byte of content and stays open, which is what starts the grace.
    let store = FsStore::load(dir.path().join("store")).await.unwrap();
    let client = endpoint::bind(
        None,
        &RelayModeChoice::Disabled,
        super::Congestion::Cubic,
        None,
    )
    .await
    .unwrap();
    let content = client
        .connect(addr.clone(), iroh_blobs::ALPN)
        .await
        .unwrap();
    for root in &roots {
        crate::engine::fetch::fetch_whole(&store, &content, Hash::from(*root))
            .await
            .unwrap();
    }
    // Every new connection below reads only the sequence, which is not content.
    let probe_store = FsStore::load(dir.path().join("probe-store")).await.unwrap();
    let probes = tokio::spawn(async move {
        for _ in 0..40 {
            if let Ok(conn) = client.connect(addr.clone(), iroh_blobs::ALPN).await {
                let _ = crate::engine::fetch::fetch_whole(&probe_store, &conn, sequence).await;
                conn.close(0u32.into(), b"probe");
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    });
    let end = tokio::time::timeout(Duration::from_secs(4), serve)
        .await
        .expect("a client that reconnected held the sender past its grace")
        .unwrap()
        .unwrap();
    assert_eq!(end, super::ServeEnd::Downloaded);
    probes.abort();
    content.close(0u32.into(), b"done");
}
