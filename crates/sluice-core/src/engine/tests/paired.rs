//! Pairing and delivery on the daemon's identity endpoint, `SPEC.md` sections 11.2 and 11.3.

use std::time::Duration;

use iroh::Endpoint;

use super::*;
use crate::device::{Devices, Grant};
use crate::engine::paired::Listener;
use crate::engine::{ALPN_DELIVER, ALPN_PAIR, InboxFolder};

/// An endpoint for a device that pairs, with no store of its own.
async fn peer() -> Endpoint {
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
    let ticket = listener.pair(Grant::Deliver, None).unwrap();
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
    let ticket = listener.pair(Grant::Deliver, Some(1)).unwrap();
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
    let ticket = listener.pair(Grant::Deliver, None).unwrap();
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
    let ticket = listener.pair(Grant::Deliver, None).unwrap();
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

/// A delivery lands in the inbox through a receive of its own, and a full inbox refuses it before
/// the first byte.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_delivery_lands_in_the_inbox_and_a_full_quota_refuses_it() {
    let dir = tempfile::tempdir().unwrap();
    let body = b"delivered through the inbox";
    let source = dir.path().join("note.txt");
    std::fs::write(&source, body).unwrap();
    let sender_home = Home::open(&dir.path().join("sender-home")).unwrap();
    let mut outgoing = super::prepare_send(
        &sender_home,
        &source,
        RelayModeChoice::Loopback,
        Congestion::Cubic,
        None,
        None,
    )
    .await
    .unwrap();
    outgoing.reserve(Keep::One);
    let text = outgoing.ticket.encode();
    let (events, _received) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        let _ = outgoing.serve(Keep::One, &events).await;
    });

    let inbox = dir.path().join("inbox");
    let home = Home::open(&dir.path().join("home")).unwrap();
    let listener = Listener::bind(
        &home,
        RelayModeChoice::Loopback,
        Some(InboxFolder {
            path: inbox.clone(),
            quota: 1 << 20,
        }),
    )
    .await
    .unwrap();
    let client = peer().await;
    let id = *client.id().as_bytes();
    listener
        .devices()
        .grant_to(id, Grant::Deliver, crate::engine::now_ms())
        .unwrap();

    assert_eq!(deliver(&client, &listener, &text).await, Some(0x00));
    let landed = inbox.join("note.txt");
    for _ in 0..100 {
        if landed.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(std::fs::read(&landed).unwrap(), body);

    // A second, tiny inbox refuses a ticket bigger than its room before the first byte.
    let small = dir.path().join("small-inbox");
    let home = Home::open(&dir.path().join("small-home")).unwrap();
    let tight = Listener::bind(
        &home,
        RelayModeChoice::Loopback,
        Some(InboxFolder {
            path: small.clone(),
            quota: 4,
        }),
    )
    .await
    .unwrap();
    tight
        .devices()
        .grant_to(id, Grant::Deliver, crate::engine::now_ms())
        .unwrap();
    assert_eq!(deliver(&client, &tight, &text).await, Some(0x01));
    assert!(!small.join("note.txt").exists());
}

/// Sends a delivery request and answers the status byte.
async fn deliver(client: &Endpoint, listener: &Listener, text: &str) -> Option<u8> {
    let conn = client
        .connect(listener.endpoint().addr(), ALPN_DELIVER)
        .await
        .unwrap();
    let (mut send, mut recv) = conn.open_bi().await.unwrap();
    let mut request = Vec::new();
    varint(&mut request, text.len() as u64);
    request.extend_from_slice(text.as_bytes());
    request.push(0);
    send.write_all(&request).await.unwrap();
    send.finish().unwrap();
    let answer = recv.read_to_end(1).await.ok();
    conn.close(0u32.into(), b"done");
    answer.and_then(|bytes| bytes.first().copied())
}

/// The request's varint, in its shortest form.
fn varint(out: &mut Vec<u8>, mut value: u64) {
    loop {
        let low = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(low);
            return;
        }
        out.push(low | 0x80);
    }
}

/// A send in the daemon's process installs its handlers on the daemon's endpoint, so the ticket
/// names the daemon's id and not a second endpoint's.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_send_attaches_to_the_daemons_endpoint() {
    let dir = tempfile::tempdir().unwrap();
    let body = b"attached to the daemon";
    let source = dir.path().join("note.txt");
    std::fs::write(&source, body).unwrap();
    let home = Home::open(&dir.path().join("home")).unwrap();
    let listener = Listener::bind(&home, RelayModeChoice::Loopback, None)
        .await
        .unwrap();
    listener.register();

    let mut outgoing = prepare_send(
        &home,
        &source,
        RelayModeChoice::Loopback,
        Congestion::Cubic,
        None,
        None,
    )
    .await
    .unwrap();
    outgoing.reserve(Keep::One);
    let text = outgoing.ticket.encode();
    let named = Ticket::decode(&text).unwrap();
    assert_eq!(named.endpoint_id(), &listener.id());
    let (events, _received) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        let _ = outgoing.serve(Keep::One, &events).await;
    });

    let out = dir.path().join("out");
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let inbound = out.clone();
    let receiving =
        tokio::spawn(async move { super::receive(&text, &options(&inbound), &tx).await });
    let received = tokio::time::timeout(Duration::from_secs(30), receiving)
        .await
        .expect("the receive ended")
        .unwrap()
        .unwrap();
    assert_eq!(received.files, 1);
    assert_eq!(std::fs::read(out.join("note.txt")).unwrap(), body);
    listener.stop();
}

/// A device with no grant is answered `0x01`, and its request is not read.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_device_without_a_grant_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let home = Home::open(&dir.path().join("home")).unwrap();
    let listener = Listener::bind(
        &home,
        RelayModeChoice::Loopback,
        Some(InboxFolder {
            path: dir.path().join("inbox"),
            quota: 1 << 20,
        }),
    )
    .await
    .unwrap();
    let client = peer().await;
    assert_eq!(
        deliver(&client, &listener, "sluice1not-a-ticket").await,
        Some(0x01)
    );
}

/// A send that attaches to the daemon serves a receiver who takes a place: the daemon's own
/// connection ids and its open and close reports reach the send's policy, which is how a limited
/// send knows which receiver took the place it offered.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_attached_limited_send_serves_one_receiver_to_completion() {
    let dir = tempfile::tempdir().unwrap();
    let body = vec![7u8; 32 << 10];
    let source = dir.path().join("limited.bin");
    std::fs::write(&source, &body).unwrap();
    let home = Home::open(&dir.path().join("home")).unwrap();
    let listener = Listener::bind(&home, RelayModeChoice::Loopback, None)
        .await
        .unwrap();
    listener.register();

    let mut outgoing = prepare_send(
        &home,
        &source,
        RelayModeChoice::Loopback,
        Congestion::Cubic,
        None,
        None,
    )
    .await
    .unwrap();
    outgoing.reserve(Keep::Receivers(1));
    let text = outgoing.ticket.encode();
    assert_eq!(Ticket::decode(&text).unwrap().endpoint_id(), &listener.id());
    let (events, _received) = tokio::sync::mpsc::unbounded_channel();
    let serving = tokio::spawn(async move { outgoing.serve(Keep::Receivers(1), &events).await });

    let out = dir.path().join("out");
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let inbound = out.clone();
    let receiving =
        tokio::spawn(async move { super::receive(&text, &options(&inbound), &tx).await });
    let received = tokio::time::timeout(Duration::from_secs(30), receiving)
        .await
        .expect("the receive ended")
        .unwrap()
        .unwrap();
    assert_eq!(received.files, 1);
    let end = tokio::time::timeout(Duration::from_secs(30), serving)
        .await
        .expect("the send did not end")
        .unwrap()
        .unwrap();
    assert_eq!(end, StopEnd::Downloaded);
    assert_eq!(std::fs::read(out.join("limited.bin")).unwrap(), body);
    listener.stop();
}
