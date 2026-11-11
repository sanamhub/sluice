//! Delivering a receipt: the receiver's ask, the sender's answer, and the receiver's rules. Each
//! case is one real transfer on loopback, so a refusal that would have stopped the content would be
//! seen here.

use std::time::Duration;

use iroh_blobs::{Hash, protocol::GetRequest, store::fs::FsStore};

use super::*;
use crate::engine::ReceiptTarget;
use crate::engine::receipts::ask_within;
use crate::receipt::Receipt;

const BODY: &[u8] = b"the content a receipt names";
/// A wait of the receiver's own, short enough that rule 6 is reached without ten seconds.
const WAIT: Duration = Duration::from_secs(2);

/// A prepared static send of [`BODY`], and the fields of its ticket a receipt is checked against.
struct Sent {
    outgoing: crate::engine::Outgoing,
    text: String,
    root: [u8; 32],
    size: u64,
    key: [u8; 32],
}

async fn sent(dir: &TempDir) -> Sent {
    let (outgoing, text) = prepared(dir, BODY).await;
    let ticket = Ticket::decode(&text).unwrap();
    let Body::Static { root, size, .. } = ticket.body() else {
        panic!("a file ticket is static");
    };
    Sent {
        outgoing,
        text,
        root: *root,
        size: *size,
        key: *ticket.endpoint_id(),
    }
}

fn with_receipt(out: &Path, receipt: &Path) -> ReceiveOptions {
    let mut options = options(out);
    options.receipt = Some(ReceiptTarget {
        path: receipt.to_path_buf(),
    });
    options
}

/// Runs `sent` to the end of one download, asking for a receipt when `ask` is set.
async fn deliver(
    dir: &TempDir,
    sent: Sent,
    ask: bool,
) -> (std::path::PathBuf, std::path::PathBuf, super::StopEnd) {
    let (serve_tx, _serve_rx) = tokio::sync::mpsc::unbounded_channel();
    let serve = tokio::spawn(async move { sent.outgoing.serve(Keep::One, &serve_tx).await });
    let out = dir.path().join("received");
    let receipt = dir.path().join("receipt.txt");
    let options = if ask {
        with_receipt(&out, &receipt)
    } else {
        options(&out)
    };
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let received = super::receive(&sent.text, &options, &tx).await.unwrap();
    assert_eq!(received.bytes, BODY.len() as u64);
    let end = tokio::time::timeout(Duration::from_secs(15), serve)
        .await
        .expect("the sender stopped")
        .unwrap()
        .unwrap();
    (out, receipt, end)
}

/// A receipt arrives after a one-download send, and it names the ticket's root, length and sender.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_receipt_arrives_after_a_one_download_send() {
    let dir = tempfile::tempdir().unwrap();
    let sent = sent(&dir).await;
    let (root, size, key) = (sent.root, sent.size, sent.key);
    let (out, receipt_path, end) = deliver(&dir, sent, true).await;
    assert_eq!(end, super::StopEnd::Downloaded);
    assert_eq!(fs::read(out.join("source.bin")).unwrap(), BODY);
    let receipt = Receipt::decode(&fs::read_to_string(&receipt_path).unwrap()).unwrap();
    receipt.verify().unwrap();
    assert_eq!(receipt.sender_key(), &key);
    assert_eq!(receipt.total_len(), size);
    assert!(receipt.is_of(&root));
    assert!(receipt.utc_ms().abs_diff(now_ms()) < 300_000);
}

/// A sender that cleared the bit writes no receipt, and the transfer still finishes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_sender_that_cleared_the_bit_answers_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let mut sent = sent(&dir).await;
    sent.outgoing.no_receipt();
    let (_out, receipt_path, end) = deliver(&dir, sent, true).await;
    assert_eq!(end, super::StopEnd::Downloaded);
    assert!(!receipt_path.exists(), "a receipt was written");
}

/// A receive that did not ask gets no receipt and does not wait for one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_receive_that_did_not_ask_gets_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let sent = sent(&dir).await;
    let (_out, receipt_path, end) = deliver(&dir, sent, false).await;
    assert_eq!(end, super::StopEnd::Downloaded);
    assert!(!receipt_path.exists(), "a receipt was written");
}

/// A stranger that knows the endpoint id and asks for the root gets nothing, and the honest
/// receiver after it still gets its receipt.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stranger_gets_nothing_and_the_receiver_after_it_does() {
    let dir = tempfile::tempdir().unwrap();
    let (outgoing, text) = prepared(&dir, BODY).await;
    let ticket = Ticket::decode(&text).unwrap();
    let Body::Static { root, size, .. } = ticket.body() else {
        panic!("a file ticket is static");
    };
    let serve = tokio::spawn(async move { outgoing.serve(Keep::One, &no_events()).await });
    let stranger = Receiver::open(RelayModeChoice::Disabled, super::Congestion::Cubic)
        .await
        .unwrap();
    assert!(
        ask_within(&stranger, &ticket, *root, *size, WAIT, &no_events())
            .await
            .is_none(),
        "a stranger was answered"
    );
    stranger.shutdown().await.unwrap();
    let out = dir.path().join("received");
    let receipt_path = dir.path().join("receipt.txt");
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    super::receive(&text, &with_receipt(&out, &receipt_path), &tx)
        .await
        .unwrap();
    let end = tokio::time::timeout(Duration::from_secs(15), serve)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(end, super::StopEnd::Downloaded);
    assert!(receipt_path.exists(), "the receiver got no receipt");
}

/// An asker that names another root than the one it was admitted for gets nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_asker_with_the_wrong_root_gets_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (outgoing, text) = prepared(&dir, BODY).await;
    let ticket = Ticket::decode(&text).unwrap();
    let Body::Static { size, .. } = ticket.body() else {
        panic!("a file ticket is static");
    };
    let serve = tokio::spawn(async move { outgoing.serve(Keep::One, &no_events()).await });
    let asker = Receiver::open(RelayModeChoice::Disabled, super::Congestion::Cubic)
        .await
        .unwrap();
    assert!(
        ask_within(&asker, &ticket, [0; 32], *size, WAIT, &no_events())
            .await
            .is_none()
    );
    asker.shutdown().await.unwrap();
    let out = dir.path().join("received");
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    super::receive(&text, &options(&out), &tx).await.unwrap();
    let end = tokio::time::timeout(Duration::from_secs(15), serve)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(end, super::StopEnd::Downloaded);
}

/// A record that breaks one receiver rule is absent, and the transfer that carried it finished:
/// one case per rule the record itself decides.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_record_that_breaks_a_rule_is_absent_and_the_transfer_finishes() {
    let now = now_ms();
    type Build = Box<dyn Fn([u8; 32], u64, [u8; 32]) -> Vec<u8>>;
    let cases: Vec<(&str, Build)> = vec![
        // Rule 1: the record's length is not 144.
        (
            "length",
            Box::new(move |_root, _size, _key| {
                let mut bytes = crate::engine::hello::varint_bytes(143);
                bytes.extend_from_slice(&[0; 143]);
                bytes
            }),
        ),
        // Rule 2: the key is not the endpoint the content came from.
        (
            "sender_key",
            Box::new(move |root, size, _key| {
                let other = Receipt::sign(
                    &ed25519_dalek::SigningKey::from_bytes(&[8; 32]),
                    crate::receipt::commitment(&root),
                    size,
                    now,
                );
                record(&other)
            }),
        ),
        // Rule 3: the commitment is another root's.
        (
            "root",
            Box::new(move |_root, size, key| {
                let other = Receipt::sign(
                    &ed25519_dalek::SigningKey::from_bytes(&key),
                    crate::receipt::commitment(&[10; 32]),
                    size,
                    now,
                );
                record(&other)
            }),
        ),
        // Rule 4: the time is outside the clock window.
        (
            "time",
            Box::new(move |root, size, key| {
                let late = Receipt::sign(
                    &ed25519_dalek::SigningKey::from_bytes(&key),
                    crate::receipt::commitment(&root),
                    size,
                    now + 300_001,
                );
                record(&late)
            }),
        ),
        // Rule 5: the signature does not hold.
        (
            "signature",
            Box::new(move |root, size, key| {
                let good = Receipt::sign(
                    &ed25519_dalek::SigningKey::from_bytes(&key),
                    crate::receipt::commitment(&root),
                    size,
                    now,
                );
                let mut bytes = record(&good);
                let at = bytes.len() - 1;
                bytes[at] ^= 1;
                bytes
            }),
        ),
    ];
    for (name, build) in cases {
        let dir = tempfile::tempdir().unwrap();
        let (outgoing, text) = prepared(&dir, BODY).await;
        let ticket = Ticket::decode(&text).unwrap();
        let Body::Static { root, size, .. } = ticket.body() else {
            panic!("a file ticket is static");
        };
        outgoing
            .provider
            .receipts()
            .force_record(build(*root, *size, *ticket.endpoint_id()));
        let serve = tokio::spawn(async move { outgoing.serve(Keep::One, &no_events()).await });
        let out = dir.path().join("received");
        let receipt_path = dir.path().join("receipt.txt");
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let received = super::receive(&text, &with_receipt(&out, &receipt_path), &tx)
            .await
            .unwrap_or_else(|e| panic!("{name}: the transfer failed: {e}"));
        assert_eq!(received.bytes, BODY.len() as u64, "{name}");
        let end = tokio::time::timeout(Duration::from_secs(15), serve)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(end, super::StopEnd::Downloaded, "{name}");
        assert!(!receipt_path.exists(), "{name}: a receipt was written");
    }
}

fn record(receipt: &Receipt) -> Vec<u8> {
    let mut bytes = crate::engine::hello::varint_bytes(144);
    bytes.extend_from_slice(&receipt.to_bytes());
    bytes
}

/// Rule 6: an asker the policy admitted, answered with nothing, waits its window and finishes with
/// no receipt. The sender keeps serving through the wait, and the transfer it already took is
/// whole.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_sender_that_writes_nothing_ends_the_wait() {
    let dir = tempfile::tempdir().unwrap();
    let (outgoing, text) = prepared(&dir, BODY).await;
    let ticket = Ticket::decode(&text).unwrap();
    let Body::Static { root, size, .. } = ticket.body() else {
        panic!("a file ticket is static");
    };
    outgoing.provider.receipts().force_record(Vec::new());
    let serve = tokio::spawn(async move { outgoing.serve(Keep::One, &no_events()).await });
    let asker = Receiver::open(RelayModeChoice::Disabled, super::Congestion::Cubic)
        .await
        .unwrap();
    // Fetch the sequence root, which is what lets the policy admit this endpoint for it.
    let conn = asker.connect_static(&ticket, no_events()).await.unwrap();
    let store = FsStore::load(dir.path().join("asker-store")).await.unwrap();
    store
        .remote()
        .execute_get(conn.clone(), GetRequest::blob(Hash::from(*root)))
        .complete()
        .await
        .unwrap();
    let started = std::time::Instant::now();
    assert!(
        ask_within(&asker, &ticket, *root, *size, WAIT, &no_events())
            .await
            .is_none()
    );
    assert!(started.elapsed() >= WAIT);
    drop(store);
    asker.shutdown().await.unwrap();
    let out = dir.path().join("received");
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    super::receive(&text, &options(&out), &tx).await.unwrap();
    let end = tokio::time::timeout(Duration::from_secs(15), serve)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(end, super::StopEnd::Downloaded);
}
