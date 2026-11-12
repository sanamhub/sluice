//! What a delta must never do, each as a transfer on loopback: tell a stranger about content, read
//! more than the ranges from a requester, believe a hostile list, or let a wrong hint decide a
//! result. Every case that fails the delta ends with the plain fetch completing the file.

use std::time::Duration;

use sluice_delta::{Entry, Params, SigList};

use super::delta::{SIZE, bytes, delta_options, sender, stopped};
use super::receipts::admit;
use super::*;
use crate::engine::hello::{CAP_DELTA, greet_stream_with, varint_bytes};

/// What a raw requester gets back: the bytes the sender writes on the greeting's stream until it
/// ends or goes quiet for a few seconds. With `uni`, that stream of its own is opened once the list
/// begins to arrive.
async fn raw(
    asker: &Receiver,
    ticket: &Ticket,
    root_bytes: &[u8],
    end: bool,
    uni: Option<Vec<u8>>,
    quiet: Duration,
) -> Vec<u8> {
    let conn = asker
        .connect_alpn(ticket, super::ALPN, no_events())
        .await
        .unwrap();
    let ours = super::Hello::ours().with(CAP_DELTA);
    let (theirs, mut send, mut recv) = greet_stream_with(&conn, ours).await.unwrap();
    assert!(
        theirs.both(&ours, CAP_DELTA),
        "the sender does not offer delta"
    );
    send.write_all(root_bytes).await.unwrap();
    if end {
        send.finish().unwrap();
    }
    let mut got = Vec::new();
    let mut piece = vec![0u8; 1 << 16];
    let mut pending = uni;
    loop {
        match tokio::time::timeout(quiet, recv.read(&mut piece)).await {
            Ok(Ok(Some(n))) => got.extend_from_slice(&piece[..n]),
            _ => break,
        }
        if let Some(stream_bytes) = pending.take() {
            let mut stream = conn.open_uni().await.unwrap();
            stream.write_all(&stream_bytes).await.unwrap();
            stream.finish().unwrap();
        }
    }
    got
}

/// How long a refusal is waited for, and how long an answer is.
const FAST: Duration = Duration::from_secs(1);
const SLOW: Duration = Duration::from_secs(5);

fn root_of(content: &[u8]) -> [u8; 32] {
    *blake3::hash(content).as_bytes()
}

fn frame_of_bytes(bytes: &[u8]) -> Vec<u8> {
    let mut out = varint_bytes(bytes.len() as u64);
    out.extend_from_slice(bytes);
    out
}

fn list_of(content: &[u8]) -> SigList {
    let params = Params::default();
    let entries = params
        .chunks(content)
        .map(|chunk| Entry::of_bytes(chunk).unwrap())
        .collect();
    SigList::new(params.avg(), entries).unwrap()
}

/// A stranger, and an asker that asks about a root that is not the content, get nothing. An asker
/// that fetched the content first is told, which shows the refusals are the policy's.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn signatures_go_only_to_an_endpoint_that_asked_for_the_content() {
    let dir = tempfile::tempdir().unwrap();
    let new = bytes(SIZE, 3);
    let root = root_of(&new);
    let sender = sender(&dir, &new).await;
    let ticket = Ticket::decode(&sender.text).unwrap();
    let stranger = Receiver::open(RelayModeChoice::Disabled, super::Congestion::Cubic)
        .await
        .unwrap();
    assert!(
        raw(&stranger, &ticket, &root, true, None, FAST)
            .await
            .is_empty()
    );
    stranger.shutdown().await.unwrap();

    let asker = Receiver::open(RelayModeChoice::Disabled, super::Congestion::Cubic)
        .await
        .unwrap();
    let _store = admit(&dir, &asker, &ticket, root).await;
    assert!(
        raw(&asker, &ticket, &[0; 32], true, None, FAST)
            .await
            .is_empty()
    );
    assert_eq!(sender.delta.served(), 0);
    let need = sluice_delta::NeedRanges::new(root, vec![])
        .unwrap()
        .encode();
    let told = raw(
        &asker,
        &ticket,
        &root,
        true,
        Some(frame_of_bytes(&need)),
        SLOW,
    )
    .await;
    // The frame is a one-byte length for a list this short, or a two-byte one for a longer list.
    let skip = if told[0] & 0x80 == 0 { 1 } else { 2 };
    let list = SigList::decode(&told[skip..]).unwrap();
    assert_eq!(list.total_len(), SIZE as u64);
    assert_eq!(sender.delta.served(), 1);
    asker.shutdown().await.unwrap();
    sender.serve.abort();
}

/// The requester sends the root and the ranges and nothing else: a byte after the root on its
/// stream is refused, and a stream of its own that is not a `NeedRanges` (its own signatures, say)
/// is read and dropped, so the ranges the sender holds stay empty.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_requester_that_sends_anything_but_ranges_is_not_believed() {
    let dir = tempfile::tempdir().unwrap();
    let new = bytes(SIZE, 4);
    let root = root_of(&new);
    let sender = sender(&dir, &new).await;
    let ticket = Ticket::decode(&sender.text).unwrap();
    let asker = Receiver::open(RelayModeChoice::Disabled, super::Congestion::Cubic)
        .await
        .unwrap();
    let _store = admit(&dir, &asker, &ticket, root).await;
    let mut more = root.to_vec();
    more.push(0);
    assert!(
        raw(&asker, &ticket, &more, true, None, FAST)
            .await
            .is_empty()
    );
    let candidate_signatures = frame_of_bytes(&list_of(&bytes(SIZE, 99)).encode());
    let told = raw(
        &asker,
        &ticket,
        &root,
        true,
        Some(candidate_signatures),
        SLOW,
    )
    .await;
    assert!(!told.is_empty(), "the list was not written");
    assert!(
        sender.delta.needs().is_empty(),
        "signatures were taken as ranges"
    );
    asker.shutdown().await.unwrap();
    sender.serve.abort();
}

/// A hostile list, each kind, costs the delta and nothing else: the file arrives whole and right,
/// every byte from the network.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_hostile_list_ends_in_a_plain_fetch_that_completes() {
    let new = bytes(SIZE, 5);
    let mut old = new.clone();
    old[1 << 20] ^= 0xff;
    let mut huge_count = vec![1u8];
    huge_count.extend_from_slice(&8192u32.to_le_bytes());
    huge_count.extend_from_slice(&varint_bytes(1 << 40));
    let mut oversized = varint_bytes(100 << 20);
    oversized.extend_from_slice(&[0; 16]);
    let cases: Vec<(&str, Vec<u8>)> = vec![
        // A length past the frame limit, refused before anything is reserved for it.
        ("a frame past the limit", oversized),
        // A count past the entry limit, refused before the entries are allocated.
        ("a count past the limit", frame_of_bytes(&huge_count)),
        // Lengths that do not add up to the content's: a list cut from other content.
        (
            "lengths of other content",
            frame_of_bytes(&list_of(&bytes(SIZE + 4096, 6)).encode()),
        ),
        // Every chunk of the old file claimed as the new file's: the matches are all false, the way
        // a colliding strong hash would make them, and only the root can tell.
        (
            "signatures of the old file",
            frame_of_bytes(&list_of(&old).encode()),
        ),
        // Bytes that are not a list at all.
        ("noise", frame_of_bytes(&[0xde, 0xad, 0xbe, 0xef, 1, 2, 3])),
    ];
    for (name, frame) in cases {
        let dir = tempfile::tempdir().unwrap();
        let sender = sender(&dir, &new).await;
        sender.delta.force_frame(frame);
        let from = dir.path().join("old.bin");
        fs::write(&from, &old).unwrap();
        let out = dir.path().join("received");
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let received = super::receive(&sender.text, &delta_options(&out, &from), &tx)
            .await
            .unwrap_or_else(|e| panic!("{name}: the receive failed: {e}"));
        assert_eq!(fs::read(out.join("source.bin")).unwrap(), new, "{name}");
        let report = received.delta.unwrap();
        assert_eq!(report.network_bytes, SIZE as u64, "{name}");
        assert_eq!(report.local_bytes, 0, "{name}");
        if name == "signatures of the old file" {
            // A list that arrived and led to a mismatch still cost its bytes.
            assert!(
                report.signature_bytes > 0,
                "{name}: the list was not counted"
            );
        }
        stopped(sender).await;
    }
}

/// A request that never ends is not answered: the sender reads one byte past the root, so a stream
/// that stays open is not the root and nothing else.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_request_that_never_ends_is_not_answered() {
    let dir = tempfile::tempdir().unwrap();
    let new = bytes(SIZE, 7);
    let root = root_of(&new);
    let sender = sender(&dir, &new).await;
    let ticket = Ticket::decode(&sender.text).unwrap();
    let asker = Receiver::open(RelayModeChoice::Disabled, super::Congestion::Cubic)
        .await
        .unwrap();
    let _store = admit(&dir, &asker, &ticket, root).await;
    assert!(
        raw(&asker, &ticket, &root, false, None, FAST)
            .await
            .is_empty()
    );
    assert_eq!(sender.delta.served(), 0);
    asker.shutdown().await.unwrap();
    sender.serve.abort();
}
