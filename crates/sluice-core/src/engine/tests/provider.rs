//! Which TLS provider the native build's own client uses, checked by negotiating with it instead of
//! reading Cargo's feature resolution.

use super::*;

/// The group a handshake settled on, read the way a probe reads one.
fn negotiated(conn: &iroh::endpoint::Connection) -> rustls::NamedGroup {
    let data = conn.handshake_data().expect("the handshake has finished");
    let data = data
        .downcast_ref::<noq::crypto::rustls::HandshakeData>()
        .expect("the handshake data is rustls'");
    data.negotiated_key_exchange_group
        .expect("the handshake records its group")
}

/// The endpoint `iroh`'s preset builds negotiates the hybrid group, so the provider the native build
/// resolves is `aws-lc-rs`. The preset names no provider and takes whichever feature is on, and it
/// prefers `ring` when both are, which has no ML-KEM group and would answer `X25519` here with no
/// error. That is what a browser crate's `tls-ring` feature joining the native build would cause,
/// and why it is not a default member.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_native_builds_default_provider_negotiates_the_post_quantum_group() {
    let dir = tempfile::tempdir().unwrap();
    let (outgoing, _text) = prepared(&dir, b"a small file").await;
    let peer = iroh::Endpoint::builder(iroh::endpoint::presets::Minimal)
        .relay_mode(iroh::RelayMode::Disabled)
        .bind_addr((std::net::Ipv4Addr::LOCALHOST, 0))
        .unwrap()
        .bind()
        .await
        .unwrap();

    let conn = peer
        .connect(outgoing.provider.addr(), iroh_blobs::ALPN)
        .await
        .unwrap();
    assert_eq!(
        format!("{:?}", negotiated(&conn)),
        "X25519MLKEM768",
        "this build's default provider is `ring`. iroh prefers it when both TLS features are on, \
         and the browser crate's `tls-ring` is what puts it in the build: `cargo nextest run \
         --workspace` builds that crate too. The gates build the default members, which leave it \
         out, and that is the build this test is about."
    );

    drop(conn);
    peer.close().await;
}
