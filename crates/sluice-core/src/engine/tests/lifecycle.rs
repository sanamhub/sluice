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
    // Windows cannot hold a time that far ahead and says so; Linux can, and keeps it. Either way
    // the file is written and nothing panics.
    #[cfg(windows)]
    assert!(warned, "the file was written without a warning");
    #[cfg(not(windows))]
    let _ = warned;
    provider.shutdown().await.unwrap();
}
