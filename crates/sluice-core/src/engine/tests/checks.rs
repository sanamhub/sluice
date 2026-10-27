//! Failure paths: a dropped provider, a refused transfer, a lying manifest and an expiry.

use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_dropped_connection_is_retried() {
    let dir = tempfile::tempdir().unwrap();
    let home = Home::open(&dir.path().join("home")).unwrap();
    let source = dir.path().join("large.bin");
    let body: Vec<u8> = (0..(4 << 20)).map(|i| (i as u8).wrapping_mul(13)).collect();
    fs::write(&source, &body).unwrap();
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
    let sequence_root = match outgoing.ticket.body() {
        Body::Static { root, .. } => *root,
        Body::Live { .. } => panic!("a live ticket"),
    };
    let sequence = outgoing
        .provider
        .store()
        .blobs()
        .get_bytes(Hash::from(sequence_root))
        .await
        .unwrap();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let out = dir.path().join("out");
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
    // A second provider on the same home and store answers the retry with the same ticket.
    let provider = restart_provider(&home, &sequence).await;
    let (received, out) = task.await.unwrap();
    let received = received.unwrap();
    assert_eq!(received.bytes, body.len() as u64);
    assert_eq!(fs::read(out.join("large.bin")).unwrap(), body);
    provider.shutdown().await.unwrap();
}

/// Waits until the transfer reports an event `wanted` accepts.
async fn wait_for(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<super::Event>,
    wanted: impl Fn(&super::Event) -> bool,
) {
    while let Some(event) = rx.recv().await {
        if wanted(&event) {
            return;
        }
    }
}

/// Starts a provider again on the home's store, answering for the roots of `sequence`.
async fn restart_provider(home: &Home, sequence: &[u8]) -> Provider {
    let allowed: BTreeSet<[u8; 32]> = crate::manifest::parse_sequence(sequence, Limits::default())
        .unwrap()
        .into_iter()
        .collect();
    let store = FsStore::load(home.path().join("blobs")).await.unwrap();
    Provider::start(
        home,
        store,
        allowed,
        RelayModeChoice::Disabled,
        super::Congestion::Cubic,
        None,
        None,
    )
    .await
    .unwrap()
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

/// A provider for a manifest that declares `declared` bytes for a blob that really has `body`.
/// Nothing checks those two agree on the sending side, because a hostile sender need not.
async fn provider_of_lying_manifest(
    dir: &TempDir,
    body: &[u8],
    declared: u64,
    mtime_ms: u64,
) -> (Provider, Hash, Ticket) {
    let home = Home::open(&dir.path().join("lying-home")).unwrap();
    let store = FsStore::load(dir.path().join("lying-store")).await.unwrap();
    let hash = store.add_slice(body).await.unwrap().hash;
    let manifest = Manifest::new(
        vec![Entry {
            path: ManifestPath::parse("big.bin").unwrap(),
            kind: Kind::File,
            size: declared,
            mtime_ms,
            mode: 0,
            loc: Loc::Blob {
                root: *hash.as_bytes(),
            },
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
    let mut allowed: BTreeSet<[u8; 32]> = manifest.hash_sequence().into_iter().collect();
    allowed.insert(manifest.sequence_root());
    let provider = Provider::start(
        &home,
        store,
        allowed,
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
            size: declared,
            format: Format::HashSequence,
        },
    )
    .unwrap();
    (provider, hash, ticket)
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
    let client_store = FsStore::load(dir.path().join("client")).await.unwrap();
    let client = endpoint::bind(
        None,
        &RelayModeChoice::Disabled,
        super::Congestion::Cubic,
        None,
    )
    .await
    .unwrap();
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
            .serve(super::Stop::OneDownload(vec![*hash.as_bytes()]))
            .await
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(300), &mut serve)
            .await
            .is_err(),
        "a partial get stopped the sender"
    );
    // Leaving is the client's own end of the transfer, which is not a partial get.
    conn.close(0u32.into(), b"done");
    let end = tokio::time::timeout(Duration::from_secs(5), serve)
        .await
        .expect("the sender did not stop after the client left")
        .unwrap()
        .unwrap();
    assert_eq!(end, super::ServeEnd::Downloaded);
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
    let client_store = FsStore::load(dir.path().join("client")).await.unwrap();
    // The client is bound before the deadline is set, so a slow machine spends its time before
    // the ticket exists rather than inside the transfer being measured.
    let client = endpoint::bind(
        None,
        &RelayModeChoice::Disabled,
        super::Congestion::Cubic,
        None,
    )
    .await
    .unwrap();
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
    let serve = tokio::spawn(async move { provider.serve(super::Stop::Keep).await });
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

/// Whether a hidden store of this transfer is still beside the output.
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
async fn a_range_request_does_not_count_as_a_complete_download() {
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
    let client_store = FsStore::load(dir.path().join("client")).await.unwrap();
    let client = endpoint::bind(
        None,
        &RelayModeChoice::Disabled,
        super::Congestion::Cubic,
        None,
    )
    .await
    .unwrap();
    let conn = client.connect(addr, iroh_blobs::ALPN).await.unwrap();
    crate::engine::fetch::fetch_range(&client_store, &conn, hash, ChunkRanges::bytes(1000..2000))
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
    let end = tokio::time::timeout(Duration::from_secs(5), serve)
        .await
        .expect("the sender did not stop after the client left")
        .unwrap()
        .unwrap();
    assert_eq!(end, super::ServeEnd::Downloaded);
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
    assert!(warned, "the file was written without a warning");
    provider.shutdown().await.unwrap();
}
