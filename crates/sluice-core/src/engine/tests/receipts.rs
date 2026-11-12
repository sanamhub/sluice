//! Delivering a receipt: the receiver's ask, the sender's answer, and the receiver's rules. Each
//! case is one real transfer on loopback, so a refusal that would have stopped the content would be
//! seen here.

use std::time::Duration;

use iroh_blobs::{Hash, protocol::GetRequest, store::fs::FsStore};

use super::*;
use crate::engine::ReceiptTarget;
use crate::engine::receipts::ask_within;
use crate::receipt::Receipt;

pub(super) const BODY: &[u8] = b"the content a receipt names";
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

/// Admits `asker` for the ticket's root, as a receiver is: it fetches the root over a content
/// connection, which is what the sender's policy records. The store holds what it fetched and has
/// to outlive the exchange.
async fn admit(dir: &TempDir, asker: &Receiver, ticket: &Ticket, root: [u8; 32]) -> FsStore {
    let conn = asker.connect_static(ticket, no_events()).await.unwrap();
    let store = FsStore::load(dir.path().join("asker-store")).await.unwrap();
    store
        .remote()
        .execute_get(conn, GetRequest::blob(Hash::from(root)))
        .complete()
        .await
        .unwrap();
    store
}

/// Sends `request` on a fresh `sluice/1` connection from `asker`, after both greetings, and says
/// whether the sender wrote anything back. With `end` the asker finishes its stream after the
/// request and the sender has five seconds to answer or close; without it the stream stays open and
/// a sender that is not answering within a second is taken as not answering.
async fn answer_to(asker: &Receiver, ticket: &Ticket, request: &[u8], end: bool) -> bool {
    let conn = asker
        .connect_alpn(ticket, super::ALPN, no_events())
        .await
        .unwrap();
    let ours = super::Hello::ours().with(crate::engine::CAP_RECEIPTS);
    let (theirs, mut send, mut recv) = crate::engine::hello::greet_stream_with(&conn, ours)
        .await
        .unwrap();
    assert!(theirs.both(&ours, crate::engine::CAP_RECEIPTS));
    tokio::io::AsyncWriteExt::write_all(&mut send, request)
        .await
        .unwrap();
    tokio::io::AsyncWriteExt::flush(&mut send).await.unwrap();
    if end {
        send.finish().unwrap();
    }
    let mut byte = [0u8; 1];
    let wait = Duration::from_secs(if end { 5 } else { 1 });
    match tokio::time::timeout(wait, recv.read_exact(&mut byte)).await {
        Ok(read) => read.is_ok(),
        Err(_) => {
            assert!(!end, "the sender neither answered nor closed the stream");
            false
        }
    }
}

/// A serving one-download sender of [`BODY`] and an asker it has admitted for the ticket's root.
/// The store and the folder have to outlive the exchange.
struct Admitted {
    serve: tokio::task::JoinHandle<Result<super::StopEnd, crate::Error>>,
    asker: Receiver,
    ticket: Ticket,
    text: String,
    root: [u8; 32],
    _store: FsStore,
    dir: TempDir,
}

/// Starts the sender and admits an asker. A positive control comes first: a valid request from
/// that asker is answered with a receipt that passes every rule, so a refusal in a test that uses
/// this is the refusal it names and not an asker that was never admitted.
async fn admitted_sender() -> Admitted {
    let dir = tempfile::tempdir().unwrap();
    let (outgoing, text) = prepared(&dir, BODY).await;
    let ticket = Ticket::decode(&text).unwrap();
    let Body::Static { root, size, .. } = ticket.body() else {
        panic!("a file ticket is static");
    };
    let (root, size) = (*root, *size);
    let serve = tokio::spawn(async move { outgoing.serve(Keep::One, &no_events()).await });
    let asker = Receiver::open(RelayModeChoice::Disabled, super::Congestion::Cubic)
        .await
        .unwrap();
    let store = admit(&dir, &asker, &ticket, root).await;
    let control = ask_within(&asker, &ticket, root, size, WAIT, &no_events())
        .await
        .expect("the admitted asker got no receipt for a valid request");
    control.verify().unwrap();
    assert!(control.is_of(&root));
    Admitted {
        serve,
        asker,
        ticket,
        text,
        root,
        _store: store,
        dir,
    }
}

pub(super) fn with_receipt(out: &Path, receipt: &Path) -> ReceiveOptions {
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

/// A sender whose answer is late still delivers it: the hold lasts until the record is written and
/// taken, not until the sender has decided to answer.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_late_answer_is_delivered_before_the_sender_stops() {
    let dir = tempfile::tempdir().unwrap();
    let sent = sent(&dir).await;
    sent.outgoing
        .provider
        .receipts()
        .delay_answers(Duration::from_millis(3000));
    let (out, receipt_path, end) = deliver(&dir, sent, true).await;
    assert_eq!(end, super::StopEnd::Downloaded);
    assert_eq!(fs::read(out.join("source.bin")).unwrap(), BODY);
    let receipt = Receipt::decode(&fs::read_to_string(&receipt_path).unwrap()).unwrap();
    receipt.verify().unwrap();
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

/// An asker the policy admitted that names another root than the ticket's gets nothing. It is
/// admitted first, and answered when it asks for the right root, so the root in the request is the
/// only reason to refuse it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_asker_with_the_wrong_root_gets_nothing() {
    let sender = admitted_sender().await;
    let mut request = crate::engine::hello::varint_bytes(32);
    request.extend_from_slice(&[0; 32]);
    assert!(
        !answer_to(&sender.asker, &sender.ticket, &request, true).await,
        "an admitted asker was answered for a root the ticket does not name"
    );
    sender.asker.shutdown().await.unwrap();
    let out = sender.dir.path().join("received");
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    super::receive(&sender.text, &options(&out), &tx)
        .await
        .unwrap();
    let end = tokio::time::timeout(Duration::from_secs(15), sender.serve)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(end, super::StopEnd::Downloaded);
}

/// A request record that is not 32 bytes is closed with nothing written, from an asker the policy
/// admitted. The length is the first thing the sender reads: the 32 bytes that follow the 31 are the
/// right root, so a sender that skipped the length check would answer.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_request_that_is_not_32_bytes_is_closed_with_nothing() {
    let sender = admitted_sender().await;
    let mut request = crate::engine::hello::varint_bytes(31);
    request.extend_from_slice(&sender.root);
    assert!(
        !answer_to(&sender.asker, &sender.ticket, &request, true).await,
        "the sender answered a request that is not 32 bytes"
    );
    sender.serve.abort();
}

/// A request whose stream goes on after the 32 bytes of the root is refused, from an asker the
/// policy admitted: the request is the whole stream, so a byte after it is not a request.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_request_followed_by_more_bytes_is_closed_with_nothing() {
    let sender = admitted_sender().await;
    let mut request = crate::engine::hello::varint_bytes(32);
    request.extend_from_slice(&sender.root);
    request.push(0);
    assert!(
        !answer_to(&sender.asker, &sender.ticket, &request, true).await,
        "the sender answered a request with a byte after the root"
    );
    sender.serve.abort();
}

/// A request that never ends is not answered: the sender waits for the end within its request
/// window and writes nothing until it comes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_request_that_never_ends_is_not_answered() {
    let sender = admitted_sender().await;
    let mut request = crate::engine::hello::varint_bytes(32);
    request.extend_from_slice(&sender.root);
    assert!(
        !answer_to(&sender.asker, &sender.ticket, &request, false).await,
        "the sender answered before the request ended"
    );
    sender.serve.abort();
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
    let store = admit(&dir, &asker, &ticket, *root).await;
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
