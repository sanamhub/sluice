//! What spends a place: a close without the completion code, a request that races the moment the
//! ticket becomes known, and a request that reaches the endpoint before the send has a mode.

use super::receivers::{client, take_whole};
use super::*;

/// One receiver that takes every byte and closes the normal way, without the completion code. It
/// holds a place all the same: the place is about who was served, not about what was said.
async fn take_whole_without_code(
    probe: &Endpoint,
    store: &FsStore,
    addr: &iroh::EndpointAddr,
    root: Hash,
) {
    let conn = probe.connect(addr.clone(), iroh_blobs::ALPN).await.unwrap();
    tokio::time::timeout(
        Duration::from_secs(20),
        crate::engine::fetch::fetch_whole(store, &conn, root),
    )
    .await
    .expect("the fetch did not finish")
    .unwrap();
    conn.close(0u32.into(), b"done");
}

/// A receive that took everything but closed without the completion code still holds a place, and
/// its close is what ends a send that offered one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_receiver_without_the_completion_code_still_uses_a_place() {
    let dir = tempfile::tempdir().unwrap();
    let body = vec![3u8; 64 << 10];
    let (outgoing, _text) = prepared(&dir, &body).await;
    let root = Hash::from(outgoing.roots[0]);
    let roots = outgoing.roots.clone();
    let addr = outgoing.provider.addr();
    let (tx, _events) = tokio::sync::mpsc::unbounded_channel();
    let serving = tokio::spawn(async move {
        outgoing
            .provider
            .serve(
                Stop::Keep {
                    limit: Some(1),
                    roots,
                },
                &tx,
            )
            .await
    });

    let (probe, store) = client(&dir, "quiet").await;
    take_whole_without_code(&probe, &store, &addr, root).await;
    let end = tokio::time::timeout(Duration::from_secs(10), serving)
        .await
        .expect("a quiet close did not end the send")
        .unwrap()
        .unwrap();
    assert_eq!(end, ServeEnd::Downloaded);
    probe.close().await;
}

/// The limit is in force before the ticket is printed. The endpoint listens from the moment the
/// send is prepared, so a request that reaches it before `serve` starts would otherwise be served
/// without a place; the sender reserves its places first, the racing endpoint takes the place, and
/// the endpoint that follows it is refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_limit_reserved_before_the_ticket_is_printed_refuses_a_racer() {
    let dir = tempfile::tempdir().unwrap();
    let body = vec![8u8; 64 << 10];
    let (mut outgoing, _text) = prepared(&dir, &body).await;
    let root = Hash::from(outgoing.roots[0]);
    let addr = outgoing.provider.addr();
    outgoing.reserve(Keep::Receivers(1));

    // The first endpoint to ask takes the place, even though `serve` has not started.
    let (racer, store) = client(&dir, "racer").await;
    let connection = racer.connect(addr.clone(), iroh_blobs::ALPN).await.unwrap();
    tokio::time::timeout(
        Duration::from_secs(20),
        crate::engine::fetch::fetch_whole(&store, &connection, root),
    )
    .await
    .expect("the racer's fetch did not finish")
    .unwrap();

    // The second one meets the reserved limit and is refused before a byte is written.
    let (late, store) = client(&dir, "late").await;
    let late_connection = late.connect(addr, iroh_blobs::ALPN).await.unwrap();
    let refused = store
        .remote()
        .execute_get(late_connection.clone(), GetRequest::blob(root))
        .complete()
        .await;
    assert!(refused.is_err(), "the reserved place did not hold");
    late_connection.close(0u32.into(), b"refused");
    late.close().await;

    // The racer's place is the only one, so its close ends the send.
    connection.close(0u32.into(), b"done");
    let (tx, _events) = tokio::sync::mpsc::unbounded_channel();
    let end = tokio::time::timeout(
        Duration::from_secs(10),
        outgoing.serve(Keep::Receivers(1), &tx),
    )
    .await
    .expect("the reserved send did not end")
    .unwrap();
    assert_eq!(end, StopEnd::Downloaded);
    racer.close().await;
}

/// The endpoint accepts from the moment the send is prepared. Until the send's mode is set a
/// request is refused and takes no place; after `reserve` the same endpoint is served, and its
/// close ends the send that offers one place.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_prepared_endpoint_serves_nothing_before_the_send_sets_its_mode() {
    let dir = tempfile::tempdir().unwrap();
    let body = vec![9u8; 64 << 10];
    let (mut outgoing, _text) = prepared_without_mode(&dir, &body).await;
    let root = Hash::from(outgoing.roots[0]);
    let addr = outgoing.provider.addr();
    let (probe, store) = client(&dir, "probe").await;

    let connection = probe.connect(addr.clone(), iroh_blobs::ALPN).await.unwrap();
    let refused = store
        .remote()
        .execute_get(connection.clone(), GetRequest::blob(root))
        .complete()
        .await;
    assert!(
        refused.is_err(),
        "a request was served before the send's mode was set"
    );
    connection.close(0u32.into(), b"refused");
    assert_eq!(outgoing.provider.served_bytes(root), 0);

    outgoing.reserve(Keep::Receivers(1));
    take_whole(&probe, &store, &addr, root).await;
    assert!(
        outgoing.provider.served_bytes(root) > 0,
        "the reserved send served nothing"
    );

    let (tx, _events) = tokio::sync::mpsc::unbounded_channel();
    let end = tokio::time::timeout(
        Duration::from_secs(10),
        outgoing.serve(Keep::Receivers(1), &tx),
    )
    .await
    .expect("the reserved send did not end")
    .unwrap();
    assert_eq!(end, StopEnd::Downloaded);
    probe.close().await;
}
