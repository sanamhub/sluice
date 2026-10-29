//! A send that serves several receivers: how many take the whole transfer, who connected, and which
//! of them counts.

use super::*;

/// A client endpoint and a store that fetch by hand, so one endpoint can connect twice and be the
/// same receiver both times.
async fn client(dir: &TempDir, name: &str) -> (Endpoint, FsStore) {
    let endpoint = endpoint::bind(None, &RelayModeChoice::Disabled, Congestion::Cubic, None)
        .await
        .unwrap();
    let store = FsStore::load(dir.path().join(format!("client-store-{name}")))
        .await
        .unwrap();
    (endpoint, store)
}

/// One receiver's whole download over `probe`, which says the transfer is complete when it is. The
/// connection is waited for after the close, so a second dial is a second connection rather than
/// the one that is still closing.
async fn take_whole(probe: &Endpoint, store: &FsStore, addr: &iroh::EndpointAddr, root: Hash) {
    let conn = probe.connect(addr.clone(), iroh_blobs::ALPN).await.unwrap();
    tokio::time::timeout(
        Duration::from_secs(20),
        crate::engine::fetch::fetch_whole(store, &conn, root),
    )
    .await
    .expect("the fetch did not finish")
    .unwrap();
    conn.close(crate::engine::COMPLETE_CLOSE.into(), b"complete");
}

/// Two receivers each take the whole ticket, and the send stops: a third is not served. The sender
/// reports one start and one finish for each, with the same id on a receiver's two lines.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_receivers_end_a_send_that_serves_that_many() {
    let dir = tempfile::tempdir().unwrap();
    let body = vec![5u8; 64 << 10];
    let (outgoing, text) = prepared(&dir, &body).await;
    let (tx, mut events) = tokio::sync::mpsc::unbounded_channel();
    let serving = tokio::spawn(async move { outgoing.serve(Keep::Receivers(2), &tx).await });

    for name in ["one", "two"] {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        super::receive(&text, &options(&dir.path().join(name)), &tx)
            .await
            .unwrap();
    }
    let end = tokio::time::timeout(Duration::from_secs(10), serving)
        .await
        .expect("the send did not stop after two receivers")
        .unwrap()
        .unwrap();
    assert_eq!(end, StopEnd::Downloaded);

    // The send has stopped, so the third ticket holder is never served: its dial does not reach a
    // sender, whether that fails now or is still waiting when the moment allowed for it passes.
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut rejected = options(&dir.path().join("three"));
    rejected.max_network_failures = 1;
    let third = tokio::time::timeout(
        Duration::from_secs(2),
        super::receive(&text, &rejected, &tx),
    )
    .await;
    assert!(
        third.is_err() || third.is_ok_and(|result| result.is_err()),
        "a third receiver was served"
    );

    assert_report_lines(&mut events, 2);
}

/// Drains the sender's receiver lines and checks them: one start and one finish per receiver, the
/// same id on a receiver's two lines, and count receivers in all.
fn assert_report_lines(events: &mut tokio::sync::mpsc::UnboundedReceiver<Event>, count: usize) {
    let mut started = 0;
    let mut finished = 0;
    let mut ids = BTreeSet::new();
    let mut closed = BTreeSet::new();
    while let Ok(event) = events.try_recv() {
        match event {
            Event::ReceiverStarted { endpoint, relayed } => {
                assert!(!relayed, "a loopback connection was reported relayed");
                started += 1;
                ids.insert(endpoint);
            }
            Event::ReceiverFinished {
                endpoint,
                relayed,
                whole,
            } => {
                assert!(!relayed, "a loopback connection was reported relayed");
                assert!(
                    whole,
                    "a receiver that took everything was reported as part"
                );
                finished += 1;
                closed.insert(endpoint);
            }
            _ => {}
        }
    }
    assert_eq!(
        (started, finished),
        (count, count),
        "one line per receiver and end"
    );
    assert_eq!(ids, closed, "a receiver's two lines name different ids");
    assert_eq!(ids.len(), count, "two receivers shared one id");
}

/// A receiver that reconnects is one receiver: two connections from one endpoint do not use up a
/// limit of two, and a second receiver's one connection does.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_receiver_that_reconnects_is_counted_once() {
    let dir = tempfile::tempdir().unwrap();
    let body = vec![6u8; 64 << 10];
    let (outgoing, _text) = prepared(&dir, &body).await;
    let root = Hash::from(outgoing.roots[0]);
    let roots = outgoing.roots.clone();
    let addr = outgoing.provider.addr();
    let (tx, mut events) = tokio::sync::mpsc::unbounded_channel();
    let mut serving = tokio::spawn(async move {
        outgoing
            .provider
            .serve(
                Stop::Keep {
                    limit: Some(2),
                    roots,
                },
                &tx,
            )
            .await
    });

    let (probe, store) = client(&dir, "probe").await;
    for _ in 0..2 {
        take_whole(&probe, &store, &addr, root).await;
        wait_for_finish(&mut events).await;
    }
    assert!(
        tokio::time::timeout(Duration::from_millis(400), &mut serving)
            .await
            .is_err(),
        "one receiver that reconnected used up a limit of two"
    );

    let (second, store) = client(&dir, "second").await;
    take_whole(&second, &store, &addr, root).await;
    let end = tokio::time::timeout(Duration::from_secs(10), serving)
        .await
        .expect("the send did not stop after the second receiver")
        .unwrap()
        .unwrap();
    assert_eq!(end, ServeEnd::Downloaded);
    probe.close().await;
    second.close().await;
}

/// A receive that took only part of a ticket closes without the completion code, so it is part of
/// the transfer and not one of the receivers the limit counts. The whole receive that follows ends
/// the send.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_partial_receive_does_not_use_up_the_limit() {
    let dir = tempfile::tempdir().unwrap();
    let tree = tree(&dir);
    let (outgoing, text) = prepared_paths(&dir, std::slice::from_ref(&tree)).await;
    let (tx, _events) = tokio::sync::mpsc::unbounded_channel();
    let serving = tokio::spawn(async move { outgoing.serve(Keep::Receivers(1), &tx).await });

    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    super::receive(
        &text,
        &selecting(&dir.path().join("part"), &["tree/a.txt"]),
        &tx,
    )
    .await
    .unwrap();
    assert!(
        !serving.is_finished(),
        "a partial receive used up the limit"
    );

    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    super::receive(&text, &options(&dir.path().join("all")), &tx)
        .await
        .unwrap();
    let end = tokio::time::timeout(Duration::from_secs(10), serving)
        .await
        .expect("the whole receive did not end the send")
        .unwrap()
        .unwrap();
    assert_eq!(end, StopEnd::Downloaded);
}

/// Waits for one receiver's finish line, so a test never sleeps between two connections.
async fn wait_for_finish(events: &mut tokio::sync::mpsc::UnboundedReceiver<Event>) {
    loop {
        let event = tokio::time::timeout(Duration::from_secs(2), events.recv())
            .await
            .expect("no receiver event arrived")
            .expect("the events channel closed");
        if matches!(event, Event::ReceiverFinished { .. }) {
            return;
        }
    }
}
