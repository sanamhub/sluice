//! The tab's hybrid group against the reference implementation, without a browser.
//!
//! A client with the provider the tab carries, which is `ring` plus RustCrypto's ML-KEM-768, and a
//! server with `aws-lc-rs`'s, which is the one the product uses, complete a handshake on loopback and
//! both say which group they agreed on. The handshake would fail on a different secret, so a passing
//! test is the combiner matching the reference: the same shares, in the same order, hashed the same
//! way.

use std::sync::Arc;

use iroh::{
    Endpoint, RelayMode,
    endpoint::{Connection, presets},
    protocol::{AcceptError, ProtocolHandler, Router},
};
use rustls::crypto::CryptoProvider;
use sluice_wasm::pq::{HYBRID_NAME, Mode, Probe, provider, wrap};

/// The ALPN a probe uses.
const ALPN: &[u8] = b"spike/pq/1";

/// Answers one byte with one byte, so the handshake is a connection both ends have.
#[derive(Debug)]
struct Echo;

impl ProtocolHandler for Echo {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let Ok((mut send, mut recv)) = connection.accept_bi().await else {
            return Ok(());
        };
        let mut bytes = [0u8; 2];
        if recv.read_exact(&mut bytes).await.is_ok() {
            let _ = send.write_all(&bytes).await;
            let _ = send.finish();
        }
        connection.closed().await;
        Ok(())
    }
}

/// The workspace's provider, every group wrapped so the probe records the one that completes.
fn aws_provider(probe: &'static Probe) -> Arc<CryptoProvider> {
    let aws = rustls::crypto::aws_lc_rs::default_provider();
    let kx_groups = aws
        .kx_groups
        .iter()
        .map(|group| wrap(*group, probe))
        .collect();
    Arc::new(CryptoProvider {
        kx_groups,
        ..aws.clone()
    })
}

#[tokio::test]
async fn the_tabs_group_is_the_one_the_reference_agrees_on() {
    let server_probe = Probe::new();
    let server = Endpoint::builder(presets::Empty)
        .crypto_provider(aws_provider(server_probe))
        .relay_mode(RelayMode::Disabled)
        .bind()
        .await
        .unwrap();
    let router = Router::builder(server.clone()).accept(ALPN, Echo).spawn();

    let client_probe = Probe::new();
    let lookup = iroh::address_lookup::MemoryLookup::new();
    lookup.add_endpoint_info(server.addr());
    let client = Endpoint::builder(presets::Empty)
        .crypto_provider(provider(Mode::Hybrid, client_probe))
        .relay_mode(RelayMode::Disabled)
        .address_lookup(lookup)
        .bind()
        .await
        .unwrap();

    let connection = client.connect(server.addr(), ALPN).await.unwrap();
    let (mut send, mut recv) = connection.open_bi().await.unwrap();
    send.write_all(b"pq").await.unwrap();
    send.finish().unwrap();
    let mut reply = [0u8; 2];
    recv.read_exact(&mut reply).await.unwrap();
    assert_eq!(&reply, b"pq");

    assert_eq!(client_probe.agreed(), Some(HYBRID_NAME));
    assert_eq!(server_probe.agreed(), Some(HYBRID_NAME));

    connection.close(0u32.into(), b"done");
    client.close().await;
    router.shutdown().await.unwrap();
}

#[tokio::test]
async fn the_classical_run_is_the_classical_group() {
    let server_probe = Probe::new();
    let server = Endpoint::builder(presets::Empty)
        .crypto_provider(aws_provider(server_probe))
        .relay_mode(RelayMode::Disabled)
        .bind()
        .await
        .unwrap();
    let router = Router::builder(server.clone()).accept(ALPN, Echo).spawn();

    let client_probe = Probe::new();
    let lookup = iroh::address_lookup::MemoryLookup::new();
    lookup.add_endpoint_info(server.addr());
    let client = Endpoint::builder(presets::Empty)
        .crypto_provider(provider(Mode::Classical, client_probe))
        .relay_mode(RelayMode::Disabled)
        .address_lookup(lookup)
        .bind()
        .await
        .unwrap();

    let connection = client.connect(server.addr(), ALPN).await.unwrap();
    connection.close(0u32.into(), b"done");

    assert_eq!(client_probe.agreed(), Some("X25519"));
    assert_eq!(server_probe.agreed(), Some("X25519"));

    client.close().await;
    router.shutdown().await.unwrap();
}
