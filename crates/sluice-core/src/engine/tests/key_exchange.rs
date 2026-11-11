//! The key exchange a connection has to settle on. A peer whose TLS offers only classical groups
//! is refused before any request is served or sent, on both sides (ADR 0018).

use std::ops::Deref;

use iroh::{Endpoint, endpoint::ConnectionError};

use super::*;

/// A peer whose TLS offers only X25519, as a build without the post-quantum provider makes. It has
/// no Sluice endpoint hook, because the peer this rule refuses is one that does not run it.
async fn classical_peer() -> Endpoint {
    let mut crypto = rustls::crypto::aws_lc_rs::default_provider();
    crypto.kx_groups = vec![rustls::crypto::aws_lc_rs::kx_group::X25519];
    Endpoint::builder(iroh::endpoint::presets::Empty)
        .crypto_provider(std::sync::Arc::new(crypto))
        .relay_mode(iroh::RelayMode::Disabled)
        .bind()
        .await
        .unwrap()
}

/// The code of a connection that a side closed by rule, when that is what ended it.
fn policy_close(error: &ConnectionError) -> Option<u32> {
    match error {
        ConnectionError::ApplicationClosed(application) => {
            Some(application.error_code.into_inner() as u32)
        }
        _ => None,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_classical_receiver_is_refused_and_the_sender_keeps_serving() {
    let dir = tempfile::tempdir().unwrap();
    let (outgoing, text) = prepared(&dir, b"the file this send offers").await;
    let addr = outgoing.provider.addr();

    let classical = classical_peer().await;
    // The sender decides after the handshake, so the dialling side can see the close either in
    // `connect` or just after it.
    match classical.connect(addr, iroh_blobs::ALPN).await {
        Ok(conn) => {
            // The sender closes a connection it will not serve, and it does so at once rather
            // than letting the peer wait for the idle timeout.
            let closed = tokio::time::timeout(std::time::Duration::from_secs(5), conn.closed())
                .await
                .expect("the sender closes a connection it will not serve");
            assert_eq!(
                policy_close(&closed),
                Some(crate::engine::POLICY_CLOSE),
                "{closed:?}"
            );
        }
        Err(error) => {
            let refused = match &error {
                iroh::endpoint::ConnectError::Connection { source, .. } => policy_close(source),
                _ => None,
            };
            assert_eq!(
                refused,
                Some(crate::engine::POLICY_CLOSE),
                "the dial was answered with {error:?}"
            );
        }
    }
    classical.close().await;

    // The same sender still serves a peer whose handshake is post-quantum.
    let (serve_tx, mut serve_rx) = tokio::sync::mpsc::unbounded_channel();
    let serve = tokio::spawn(async move { outgoing.serve(Keep::One, &serve_tx).await });
    let out = dir.path().join("received");
    let (recv_tx, _recv_rx) = tokio::sync::mpsc::unbounded_channel();
    let received = super::receive(&text, &options(&out), &recv_tx)
        .await
        .unwrap();
    assert_eq!(
        fs::read(received.path).unwrap(),
        b"the file this send offers"
    );
    assert_eq!(serve.await.unwrap().unwrap(), super::StopEnd::Downloaded);

    // The line the sender prints names the exact group it got. `X25519` is a prefix of the
    // required group's name, so a test that looks for both names in a line passes whatever the
    // sender negotiated; the line has to be matched as one phrase instead.
    let mut lines = Vec::new();
    while let Ok(event) = serve_rx.try_recv() {
        if let Event::Warning { message } = event {
            lines.push(message);
        }
    }
    assert_eq!(lines.len(), 1, "{lines:?}");
    let line = &lines[0];
    assert!(
        line.contains("negotiated X25519,") && line.contains("requires X25519MLKEM768"),
        "{line}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn refusals_from_fresh_endpoints_stop_after_the_cap() {
    let dir = tempfile::tempdir().unwrap();
    let (outgoing, text) = prepared(&dir, b"the file this send offers").await;
    let addr = outgoing.provider.addr();
    let (serve_tx, mut serve_rx) = tokio::sync::mpsc::unbounded_channel();
    let serve = tokio::spawn(async move { outgoing.serve(Keep::One, &serve_tx).await });

    // Five peers, each a fresh endpoint id, as a peer that makes a new identity per try would be.
    for _ in 0..5 {
        let classical = classical_peer().await;
        let _ = classical.connect(addr.clone(), iroh_blobs::ALPN).await;
        classical.close().await;
    }

    let out = dir.path().join("received");
    let (recv_tx, _recv_rx) = tokio::sync::mpsc::unbounded_channel();
    super::receive(&text, &options(&out), &recv_tx)
        .await
        .unwrap();
    assert_eq!(serve.await.unwrap().unwrap(), super::StopEnd::Downloaded);

    let lines = std::iter::from_fn(|| serve_rx.try_recv().ok())
        .filter(|event| matches!(event, Event::Warning { .. }))
        .count();
    assert_eq!(lines, crate::engine::policy::REFUSAL_LINES);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_receiver_refuses_a_classical_sender() {
    let classical = classical_peer().await;
    let store = iroh_blobs::store::mem::MemStore::new();
    let router = iroh::protocol::Router::builder(classical.clone())
        .accept(
            iroh_blobs::ALPN,
            iroh_blobs::BlobsProtocol::new(store.deref(), None),
        )
        .spawn();
    let ticket = Ticket::new(
        *classical.id().as_bytes(),
        classical.addr().ip_addrs().copied().collect(),
        None,
        None,
        Body::Static {
            root: [0u8; 32],
            size: 1,
            format: Format::HashSequence,
        },
    )
    .unwrap();

    let receiver = Receiver::open(RelayModeChoice::Disabled, Congestion::Cubic)
        .await
        .unwrap();
    let error = receiver.connect(&ticket, no_events()).await.unwrap_err();
    let Error::Policy(message) = &error else {
        panic!("a classical sender has to be a policy refusal: {error}");
    };
    assert!(message.contains("X25519"), "{message}");
    assert!(message.contains("X25519MLKEM768"), "{message}");

    router.shutdown().await.unwrap();
    receiver.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_normal_peers_negotiate_the_post_quantum_group() {
    let dir = tempfile::tempdir().unwrap();
    let (_outgoing, text) = prepared(&dir, b"a small file").await;
    let ticket = Ticket::decode(&text).unwrap();

    let receiver = Receiver::open(RelayModeChoice::Disabled, Congestion::Cubic)
        .await
        .unwrap();
    let conn = receiver.connect(&ticket, no_events()).await.unwrap();
    let group = endpoint::negotiated_group(&conn).expect("the handshake records its group");
    assert_eq!(format!("{group:?}"), "X25519MLKEM768");

    drop(conn);
    receiver.shutdown().await.unwrap();
}
