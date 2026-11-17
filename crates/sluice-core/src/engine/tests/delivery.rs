//! A delivery: the request, the top-level name it lands under, the quota, and the endpoint the
//! receive uses. `SPEC.md` section 11.3.

use super::paired::peer;
use super::*;
use crate::device::Grant;
use crate::engine::{ALPN_DELIVER, InboxFolder, Listener};

/// A delivery is a receive of its own inside a top-level entry of the inbox, and a full inbox
/// refuses it before the first byte.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_delivery_lands_under_the_roots_hex_and_a_full_quota_refuses_it() {
    let dir = tempfile::tempdir().unwrap();
    let (client, listener, text) = delivery_at(&dir, "inbox", 1 << 20).await;
    assert_eq!(deliver(&client, &listener, &text).await, Some(0x00));
    let landed = dir
        .path()
        .join("inbox")
        .join(root_hex(&text))
        .join("note.txt");
    wait_for(&landed).await;
    assert_eq!(
        std::fs::read(&landed).unwrap(),
        b"delivered through the inbox"
    );

    // A second, tiny inbox refuses a ticket bigger than its room before the first byte.
    let (client, tight) = granted_listener(&dir, "small", 4).await;
    assert_eq!(deliver(&client, &tight, &text).await, Some(0x01));
    assert!(
        !dir.path().join("small").join(root_hex(&text)).exists(),
        "a refused delivery left its entry behind"
    );
}

/// A delivery that names its top-level entry lands under that name.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_named_delivery_lands_under_the_name() {
    let dir = tempfile::tempdir().unwrap();
    let (client, listener, text) = delivery_at(&dir, "inbox", 1 << 20).await;
    assert_eq!(
        deliver_named(&client, &listener, &text, Some("report")).await,
        Some(0x00)
    );
    let landed = dir.path().join("inbox").join("report").join("note.txt");
    wait_for(&landed).await;
    assert_eq!(
        std::fs::read(&landed).unwrap(),
        b"delivered through the inbox"
    );
}

/// A name the path rules of section 6 refuse is refused, and the inbox does not grow an entry for
/// it. The rules cover a path separator, `..`, a control character, an empty name, a trailing
/// space and a Windows device name.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_delivery_name_the_path_rules_refuse_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (client, listener, text) = delivery_at(&dir, "inbox", 1 << 20).await;
    for name in ["a/b", "..", "a\u{7}b", "", "a ", "CON"] {
        assert_eq!(
            deliver_named(&client, &listener, &text, Some(name)).await,
            Some(0x01),
            "{name:?} was not refused"
        );
    }
    assert_eq!(
        std::fs::read_dir(dir.path().join("inbox")).unwrap().count(),
        0,
        "a refused delivery left an entry behind"
    );
}

/// A top-level entry that is already in the inbox is refused, and a delivery never replaces it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_delivery_name_that_is_already_there_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (client, listener, text) = delivery_at(&dir, "inbox", 1 << 20).await;
    std::fs::create_dir(dir.path().join("inbox").join("taken")).unwrap();
    assert_eq!(
        deliver_named(&client, &listener, &text, Some("taken")).await,
        Some(0x01)
    );
    assert!(
        std::fs::read_dir(dir.path().join("inbox").join("taken"))
            .unwrap()
            .next()
            .is_none(),
        "a refused delivery wrote into the entry that was there"
    );

    assert_eq!(
        deliver_named(&client, &listener, &text, Some("report")).await,
        Some(0x00)
    );
    wait_for(&dir.path().join("inbox/report/note.txt")).await;
    assert_eq!(
        deliver_named(&client, &listener, &text, Some("report")).await,
        Some(0x01)
    );
}

/// A live ticket is refused before any byte: a stream states no size, so it cannot be reserved.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_live_ticket_is_refused_before_any_byte() {
    let dir = tempfile::tempdir().unwrap();
    let (client, listener) = granted_listener(&dir, "inbox", 1 << 20).await;
    let live = Ticket::new(
        listener.id(),
        vec![],
        None,
        Some(crate::engine::now_ms() + 60_000),
        Body::Live {
            stream_id: [1; 16],
            sender_key: *iroh::SecretKey::generate().public().as_bytes(),
        },
    )
    .unwrap()
    .encode();
    assert_eq!(
        deliver_named(&client, &listener, &live, Some("stream")).await,
        Some(0x01)
    );
    assert!(!dir.path().join("inbox").join("stream").exists());
}

/// A listener with an inbox, and a device granted `deliver`.
async fn granted_listener(
    dir: &TempDir,
    inbox: &str,
    quota: u64,
) -> (Endpoint, std::sync::Arc<Listener>) {
    let home = Home::open(&dir.path().join(format!("home-{inbox}"))).unwrap();
    let listener = Listener::bind(
        &home,
        RelayModeChoice::Loopback,
        Some(InboxFolder {
            path: dir.path().join(inbox),
            quota,
        }),
    )
    .await
    .unwrap();
    let client = peer().await;
    listener
        .devices()
        .grant_to(
            *client.id().as_bytes(),
            Grant::Deliver,
            crate::engine::now_ms(),
            None,
        )
        .unwrap();
    (client, listener)
}

/// A sender serving one file, and the ticket it serves.
async fn sending(dir: &TempDir, tag: &str) -> String {
    let source = dir.path().join("note.txt");
    std::fs::write(&source, b"delivered through the inbox").unwrap();
    let home = Home::open(&dir.path().join(format!("sender-{tag}"))).unwrap();
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
    let (events, _received) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        let _ = outgoing.serve(Keep::One, &events).await;
    });
    text
}

/// A listener with an inbox, a device granted `deliver`, and a sender serving one file.
async fn delivery_at(
    dir: &TempDir,
    inbox: &str,
    quota: u64,
) -> (Endpoint, std::sync::Arc<Listener>, String) {
    let (client, listener) = granted_listener(dir, inbox, quota).await;
    let text = sending(dir, inbox).await;
    (client, listener, text)
}

/// The ticket's root in lower-case hex, which is the name a delivery without one lands under.
fn root_hex(text: &str) -> String {
    match Ticket::decode(text).unwrap().body() {
        Body::Static { root, .. } => root.iter().map(|byte| format!("{byte:02x}")).collect(),
        other => panic!("not a static ticket: {other:?}"),
    }
}

/// Waits for a delivery's receive to write `path`, which runs after the status byte.
async fn wait_for(path: &std::path::Path) {
    for _ in 0..100 {
        if path.exists() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("{} did not appear", path.display());
}

/// Sends a delivery request with no name and answers the status byte.
async fn deliver(client: &Endpoint, listener: &Listener, text: &str) -> Option<u8> {
    deliver_named(client, listener, text, None).await
}

/// Sends a delivery request and answers the status byte.
async fn deliver_named(
    client: &Endpoint,
    listener: &Listener,
    text: &str,
    name: Option<&str>,
) -> Option<u8> {
    let conn = client
        .connect(listener.endpoint().addr(), ALPN_DELIVER)
        .await
        .unwrap();
    let (mut send, mut recv) = conn.open_bi().await.unwrap();
    let mut request = Vec::new();
    varint(&mut request, text.len() as u64);
    request.extend_from_slice(text.as_bytes());
    match name {
        None => request.push(0),
        Some(name) => {
            request.push(1);
            varint(&mut request, name.len() as u64);
            request.extend_from_slice(name.as_bytes());
        }
    }
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

/// A delivery's receive binds an endpoint of its own, so the content connection the sender sees
/// comes from an endpoint that is not the daemon's identity endpoint.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_deliverys_receive_uses_an_endpoint_of_its_own() {
    let dir = tempfile::tempdir().unwrap();
    let (client, listener) = granted_listener(&dir, "inbox", 1 << 20).await;
    let source = dir.path().join("note.txt");
    std::fs::write(&source, b"delivered through the inbox").unwrap();
    let home = Home::open(&dir.path().join("sender")).unwrap();
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
    let asked = outgoing.provider.asked();
    let (events, _rx) = tokio::sync::mpsc::unbounded_channel();
    let serving = tokio::spawn(async move { outgoing.serve(Keep::One, &events).await });
    assert_eq!(deliver(&client, &listener, &text).await, Some(0x00));
    wait_for(
        &dir.path()
            .join("inbox")
            .join(root_hex(&text))
            .join("note.txt"),
    )
    .await;
    let end = tokio::time::timeout(Duration::from_secs(30), serving)
        .await
        .expect("the send ended")
        .unwrap()
        .unwrap();
    assert_eq!(end, StopEnd::Downloaded);
    let seen = asked
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .peers();
    assert!(!seen.is_empty(), "the sender saw no receiver");
    assert!(
        !seen.contains(&listener.id()),
        "the delivery received under the daemon's own endpoint"
    );
}
