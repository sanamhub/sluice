//! Pairing and delivery on the daemon's identity endpoint, `SPEC.md` sections 11.2 and 11.3.

use std::time::Duration;

use iroh::Endpoint;

use super::*;
use crate::device::{Devices, Grant};
use crate::engine::ALPN_PAIR;
use crate::engine::paired::Listener;

/// An endpoint for a device that pairs, with no store of its own.
pub(super) async fn peer() -> Endpoint {
    endpoint::bind(None, &RelayModeChoice::Disabled, Congestion::Cubic, None)
        .await
        .unwrap()
}

/// The secret a pairing ticket carries.
fn secret(text: &str) -> [u8; 32] {
    match Ticket::decode(text).unwrap().body() {
        Body::Pairing { secret, .. } => *secret,
        other => panic!("not a pairing ticket: {other:?}"),
    }
}

/// Redeems `text` from `client` and answers the status byte, or `None` when the connection ended
/// before one arrived.
async fn redeem(client: &Endpoint, listener: &Listener, text: &str) -> Option<u8> {
    let conn = client
        .connect(listener.endpoint().addr(), ALPN_PAIR)
        .await
        .unwrap();
    let (mut send, mut recv) = conn.open_bi().await.unwrap();
    send.write_all(&secret(text)).await.unwrap();
    send.finish().unwrap();
    let answer = recv.read_to_end(1).await.ok();
    conn.close(0u32.into(), b"done");
    answer.and_then(|bytes| bytes.first().copied())
}

/// A loopback listener over a fresh home.
async fn listener(dir: &TempDir) -> (Home, std::sync::Arc<Listener>) {
    let home = Home::open(&dir.path().join("home")).unwrap();
    let listener = Listener::bind(&home, RelayModeChoice::Loopback, None)
        .await
        .unwrap();
    (home, listener)
}

/// A pairing ticket grants `deliver`, a second redemption by the same device answers success, and
/// the grant is on disk for the next daemon.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_pairing_ticket_grants_deliver_and_the_grant_is_on_disk() {
    let dir = tempfile::tempdir().unwrap();
    let (home, listener) = listener(&dir).await;
    let ticket = listener.pair(Grant::Deliver, None, None).unwrap();
    let client = peer().await;
    assert_eq!(redeem(&client, &listener, &ticket.text).await, Some(0x00));
    let id = *client.id().as_bytes();
    assert_eq!(listener.devices().grant(&id), Some(Grant::Deliver));
    assert_eq!(redeem(&client, &listener, &ticket.text).await, Some(0x00));
    // The next daemon reads the grant from the file, not from this process's map.
    let reopened = Devices::open(home.path());
    assert_eq!(reopened.grant(&id), Some(Grant::Deliver));
    assert_eq!(
        reopened.list().unwrap()[0].label,
        crate::device::default_label(&id)
    );
}

/// A pairing ticket is single use: another endpoint id that presents it is refused, and an expired
/// one is refused too.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_ticket_belongs_to_the_first_device_and_expires() {
    let dir = tempfile::tempdir().unwrap();
    let (_home, listener) = listener(&dir).await;
    let ticket = listener.pair(Grant::Deliver, Some(1), None).unwrap();
    let first = peer().await;
    let second = peer().await;
    assert_eq!(redeem(&first, &listener, &ticket.text).await, Some(0x00));
    assert_eq!(redeem(&second, &listener, &ticket.text).await, Some(0x01));
    assert_eq!(
        listener.devices().grant(second.id().as_bytes()),
        None,
        "a refused device holds nothing"
    );
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let third = peer().await;
    assert_eq!(redeem(&third, &listener, &ticket.text).await, Some(0x01));
    assert_eq!(listener.devices().list().unwrap().len(), 1);
}

/// A revoke forgets the ticket that made the grant, so a retry with that ticket is refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_revoke_forgets_the_ticket_that_made_the_grant() {
    let dir = tempfile::tempdir().unwrap();
    let (_home, listener) = listener(&dir).await;
    let ticket = listener.pair(Grant::Deliver, None, None).unwrap();
    let client = peer().await;
    let id = *client.id().as_bytes();
    assert_eq!(redeem(&client, &listener, &ticket.text).await, Some(0x00));
    assert!(listener.revoke(&id).unwrap());
    assert_eq!(listener.devices().grant(&id), None);
    assert_eq!(redeem(&client, &listener, &ticket.text).await, Some(0x01));
    assert!(!listener.revoke(&id).unwrap(), "nothing left to revoke");
}

/// A grants file the daemon cannot read refuses every pairing attempt with `0x02` and is left as
/// it was.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_grants_file_that_cannot_be_read_refuses_and_is_left_alone() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("home")).unwrap();
    let broken = dir.path().join("home").join("devices.json");
    std::fs::write(&broken, b"not a grants file").unwrap();
    let (home, listener) = listener(&dir).await;
    let ticket = listener.pair(Grant::Deliver, None, None).unwrap();
    let client = peer().await;
    assert_eq!(redeem(&client, &listener, &ticket.text).await, Some(0x02));
    assert!(listener.devices().list().is_err());
    assert_eq!(std::fs::read(&broken).unwrap(), b"not a grants file");
    drop(home);
}

/// The fifth pairing stream from one endpoint id is closed, and the four before it are served.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_fifth_pairing_stream_from_one_endpoint_is_closed() {
    let dir = tempfile::tempdir().unwrap();
    let (_home, listener) = listener(&dir).await;
    let client = peer().await;
    let addr = listener.endpoint().addr();
    let mut held = Vec::new();
    for _ in 0..4 {
        let conn = client.connect(addr.clone(), ALPN_PAIR).await.unwrap();
        let (mut send, _recv) = conn.open_bi().await.unwrap();
        // One byte and no finish: the handler is still waiting for the 32 bytes.
        send.write_all(&[0]).await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        held.push(conn);
    }
    // The fifth is closed before it is served: the daemon answers with no status byte at all.
    let fifth = client.connect(addr, ALPN_PAIR).await;
    let refused = match fifth {
        Err(_) => true,
        Ok(conn) => match conn.open_bi().await {
            Err(_) => true,
            Ok((mut send, mut recv)) => {
                let _ = send.write_all(&[0]).await;
                let _ = send.finish();
                match tokio::time::timeout(Duration::from_secs(5), recv.read_to_end(1)).await {
                    Ok(Ok(bytes)) => bytes.is_empty(),
                    _ => true,
                }
            }
        },
    };
    assert!(refused, "the fifth stream is refused");
    // The four that were there first end when their endpoints go.
    for conn in held {
        conn.close(0u32.into(), b"done");
    }
}
