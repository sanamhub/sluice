//! The sender's cache and the receiver's cut under the cases an audit raised: a peer's frame limit,
//! a cut that outlives its attempt, a build that stops in each of its stages, a build that has done
//! half its work, and ranges that were fetched before a hash did not match.

use std::time::Duration;

use sluice_delta::{Entry, Params, SigList};

use super::delta::{SIZE, bytes, delta_options, sender, stopped};
use super::*;
use crate::engine::hello::{CAP_DELTA, greet_stream_with, varint_bytes};

/// What the sender writes in answer to a request for `root` from a requester that says its frames
/// may be `limit` bytes, waiting `wait` for it. Dropping the connection is leaving.
async fn ask_with(
    asker: &Receiver,
    ticket: &Ticket,
    root: [u8; 32],
    limit: u32,
    wait: Duration,
) -> (iroh::endpoint::Connection, Vec<u8>) {
    let conn = asker
        .connect_alpn(ticket, super::ALPN, no_events())
        .await
        .unwrap();
    let mut ours = super::Hello::ours().with(CAP_DELTA);
    ours.max_frame_bytes = limit;
    let (_theirs, mut send, mut recv) = greet_stream_with(&conn, ours).await.unwrap();
    send.write_all(&root).await.unwrap();
    send.finish().unwrap();
    let mut got = Vec::new();
    let mut piece = vec![0u8; 1 << 16];
    while let Ok(Ok(Some(n))) = tokio::time::timeout(wait, recv.read(&mut piece)).await {
        got.extend_from_slice(&piece[..n]);
    }
    (conn, got)
}

async fn admitted(dir: &TempDir, asker: &Receiver, ticket: &Ticket, root: [u8; 32]) -> FsStore {
    super::receipts::admit(dir, asker, ticket, root).await
}

async fn lasting(dir: &TempDir, new: &[u8]) -> super::delta::Sender {
    let (outgoing, text) = prepared(dir, new).await;
    let delta = std::sync::Arc::clone(outgoing.provider.delta().unwrap());
    let serve = tokio::spawn(async move { outgoing.serve(Keep::UntilStopped, &no_events()).await });
    super::delta::Sender { text, serve, delta }
}

async fn eventually(what: &str, mut ready: impl FnMut() -> bool) {
    let until = std::time::Instant::now() + Duration::from_secs(10);
    while !ready() {
        assert!(std::time::Instant::now() < until, "{what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn list_frame(content: &[u8], avg: u32) -> Vec<u8> {
    let params = Params::new(avg).unwrap();
    let entries = params
        .chunks(content)
        .map(|chunk| Entry::of_bytes(chunk).unwrap())
        .collect();
    let list = SigList::new(avg, entries).unwrap();
    let mut out = varint_bytes(list.encode().len() as u64);
    out.extend_from_slice(&list.encode());
    out
}

/// What a peer says about its frame limit decides nothing about the list: a requester that says 1
/// is refused, one that says `u32::MAX` is given the same list a normal requester is, and the list
/// is built once, with this side's own limit.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_peers_frame_limit_does_not_decide_the_cached_list() {
    let dir = tempfile::tempdir().unwrap();
    let new = bytes(SIZE, 51);
    let root = *blake3::hash(&new).as_bytes();
    let sender = lasting(&dir, &new).await;
    let ticket = Ticket::decode(&sender.text).unwrap();
    let asker = Receiver::open(RelayModeChoice::Disabled, super::Congestion::Cubic)
        .await
        .unwrap();
    let _store = admitted(&dir, &asker, &ticket, root).await;
    let wait = Duration::from_secs(5);
    let (_a, small) = ask_with(&asker, &ticket, root, 1, wait).await;
    assert!(small.is_empty(), "a list was written for a limit of 1");
    let (_b, normal) = ask_with(&asker, &ticket, root, 16 << 20, wait).await;
    assert!(
        !normal.is_empty(),
        "the limit of the first requester was cached"
    );
    let (_c, huge) = ask_with(&asker, &ticket, root, u32::MAX, wait).await;
    assert_eq!(normal, huge);
    assert!(normal.len() < 16 << 20);
    assert_eq!(sender.delta.builds(), 1);
    asker.shutdown().await.unwrap();
    sender.serve.abort();
}

/// A receive in place from a sender that does not offer delta: the early cut of the candidate is
/// stopped and has let go of the file before the output is written over it, which a handle still
/// open would stop on Windows.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cut_started_for_a_sender_without_delta_is_stopped_before_the_write() {
    let dir = tempfile::tempdir().unwrap();
    let new = bytes(SIZE, 52);
    let mut old = new.clone();
    old[1 << 20] ^= 1;
    let out = dir.path().join("received");
    fs::create_dir_all(&out).unwrap();
    let target = out.join("source.bin");
    fs::write(&target, &old).unwrap();
    let sender = sender(&dir, &new).await;
    sender.delta.stop_offering();
    let mut options = delta_options(&out, &target);
    options.force = true;
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let received = super::receive(&sender.text, &options, &tx).await.unwrap();
    assert_eq!(fs::read(&target).unwrap(), new);
    let report = received.delta.unwrap();
    assert_eq!((report.local_bytes, report.network_bytes), (0, SIZE as u64));
    assert_eq!(sender.delta.served(), 0);
    stopped(sender).await;
}

/// A list cut with an average other than the receiver's guess replaces the early cut, which is
/// stopped; the update in place still goes through, and the delta is used.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_average_that_differs_from_the_guess_replaces_the_early_cut() {
    let dir = tempfile::tempdir().unwrap();
    let new = bytes(SIZE, 53);
    let mut old = new.clone();
    for byte in &mut old[1 << 20..(1 << 20) + 50_000] {
        *byte ^= 0x3c;
    }
    let out = dir.path().join("received");
    fs::create_dir_all(&out).unwrap();
    let target = out.join("source.bin");
    fs::write(&target, &old).unwrap();
    let sender = sender(&dir, &new).await;
    sender.delta.force_frame(list_frame(&new, 16_384));
    let mut options = delta_options(&out, &target);
    options.force = true;
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let received = super::receive(&sender.text, &options, &tx).await.unwrap();
    assert_eq!(fs::read(&target).unwrap(), new);
    let report = received.delta.unwrap();
    assert!(report.local_bytes > SIZE as u64 / 2, "{report:?}");
    stopped(sender).await;
}

/// A build stops in the reader when its requester leaves early: it gives its permit back and no
/// list is cached.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_build_stops_in_the_reader_when_its_requester_leaves() {
    let dir = tempfile::tempdir().unwrap();
    let new = bytes(12 << 20, 54);
    let root = *blake3::hash(&new).as_bytes();
    let sender = lasting(&dir, &new).await;
    sender
        .delta
        .delays(Duration::ZERO, Duration::from_millis(200), Duration::ZERO);
    let ticket = Ticket::decode(&sender.text).unwrap();
    let asker = Receiver::open(RelayModeChoice::Disabled, super::Congestion::Cubic)
        .await
        .unwrap();
    let _store = admitted(&dir, &asker, &ticket, root).await;
    let (conn, got) = ask_with(&asker, &ticket, root, 16 << 20, Duration::from_millis(500)).await;
    assert!(got.is_empty());
    conn.close(0u32.into(), b"");
    let delta = std::sync::Arc::clone(&sender.delta);
    eventually("the reader did not stop", move || delta.stopped_in().0 == 1).await;
    let delta = std::sync::Arc::clone(&sender.delta);
    eventually("the permit was not given back", move || {
        delta.free_permits() == 2
    })
    .await;
    asker.shutdown().await.unwrap();
    sender.serve.abort();
}

/// The same for the thread that hashes: a delay for each block it takes lets the stop be seen
/// there. The reader runs ahead by the queue, so the blob is long enough that it is not yet half
/// read when the requester leaves.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_build_stops_in_the_hashing_thread_when_its_requester_leaves() {
    let dir = tempfile::tempdir().unwrap();
    let new = bytes(24 << 20, 55);
    let root = *blake3::hash(&new).as_bytes();
    let sender = lasting(&dir, &new).await;
    sender
        .delta
        .delays(Duration::ZERO, Duration::ZERO, Duration::from_millis(300));
    let ticket = Ticket::decode(&sender.text).unwrap();
    let asker = Receiver::open(RelayModeChoice::Disabled, super::Congestion::Cubic)
        .await
        .unwrap();
    let _store = admitted(&dir, &asker, &ticket, root).await;
    let (conn, got) = ask_with(&asker, &ticket, root, 16 << 20, Duration::from_millis(600)).await;
    assert!(got.is_empty());
    conn.close(0u32.into(), b"");
    let delta = std::sync::Arc::clone(&sender.delta);
    eventually("the hashing thread did not stop", move || {
        delta.stopped_in().1 == 1
    })
    .await;
    let delta = std::sync::Arc::clone(&sender.delta);
    eventually("the permit was not given back", move || {
        delta.free_permits() == 2
    })
    .await;
    asker.shutdown().await.unwrap();
    sender.serve.abort();
}

/// A build past half of its blob keeps going when its requester leaves, and caches its list: the
/// next request is served from it with no second build.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_build_past_half_finishes_and_is_cached_when_its_requester_leaves() {
    let dir = tempfile::tempdir().unwrap();
    let new = bytes(8 << 20, 56);
    let root = *blake3::hash(&new).as_bytes();
    let sender = lasting(&dir, &new).await;
    sender
        .delta
        .delays(Duration::ZERO, Duration::from_millis(300), Duration::ZERO);
    let ticket = Ticket::decode(&sender.text).unwrap();
    let asker = Receiver::open(RelayModeChoice::Disabled, super::Congestion::Cubic)
        .await
        .unwrap();
    let _store = admitted(&dir, &asker, &ticket, root).await;
    // Eight blocks at 300 ms: the requester leaves after five, past half.
    let (conn, got) = ask_with(&asker, &ticket, root, 16 << 20, Duration::from_millis(1500)).await;
    assert!(got.is_empty());
    conn.close(0u32.into(), b"");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(sender.delta.free_permits(), 1, "the build was stopped");
    let delta = std::sync::Arc::clone(&sender.delta);
    eventually("the build did not finish", move || {
        delta.free_permits() == 2
    })
    .await;
    let (_again, told) = ask_with(&asker, &ticket, root, 16 << 20, Duration::from_secs(5)).await;
    assert!(!told.is_empty(), "the list was not cached");
    assert_eq!(sender.delta.builds(), 1);
    asker.shutdown().await.unwrap();
    sender.serve.abort();
}

/// Ranges fetched before the file fails its hash stay counted: the receive reports the whole file
/// from the network and the ranges as well.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ranges_fetched_before_a_mismatch_stay_counted() {
    let dir = tempfile::tempdir().unwrap();
    let new = bytes(SIZE, 57);
    let (c, d, len) = (512 << 10, 2 << 20, 64 << 10);
    let mut candidate = new.clone();
    candidate[c..c + len].iter_mut().for_each(|b| *b ^= 0x55);
    candidate[d..d + len].iter_mut().for_each(|b| *b ^= 0x66);
    // The list claims the candidate's bytes at `c` and the real ones at `d`: the chunks at `c`
    // match and are copied wrong, the chunks at `d` do not match and are fetched.
    let mut claimed = new.clone();
    claimed[c..c + len].iter_mut().for_each(|b| *b ^= 0x55);
    let from = dir.path().join("old.bin");
    fs::write(&from, &candidate).unwrap();
    let sender = sender(&dir, &new).await;
    sender.delta.force_frame(list_frame(&claimed, 8192));
    let out = dir.path().join("received");
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let received = super::receive(&sender.text, &delta_options(&out, &from), &tx)
        .await
        .unwrap();
    assert_eq!(fs::read(out.join("source.bin")).unwrap(), new);
    let report = received.delta.unwrap();
    assert_eq!(
        report.local_bytes, 0,
        "a mismatched file counts nothing local"
    );
    let extra = report.network_bytes - SIZE as u64;
    assert!(extra >= len as u64 && extra < SIZE as u64 / 4, "{report:?}");
    stopped(sender).await;
}

/// A build that fails on something that may not recur (here a work folder that cannot be made) is
/// not cached: once the folder is there the next request builds the list.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_build_is_not_cached() {
    let dir = tempfile::tempdir().unwrap();
    let new = bytes(SIZE, 58);
    let root = *blake3::hash(&new).as_bytes();
    let sender = lasting(&dir, &new).await;
    let blocked = dir.path().join("a-file");
    fs::write(&blocked, b"x").unwrap();
    sender.delta.set_work(blocked.join("under-a-file"));
    let ticket = Ticket::decode(&sender.text).unwrap();
    let asker = Receiver::open(RelayModeChoice::Disabled, super::Congestion::Cubic)
        .await
        .unwrap();
    let _store = admitted(&dir, &asker, &ticket, root).await;
    let (_a, none) = ask_with(&asker, &ticket, root, 16 << 20, Duration::from_secs(5)).await;
    assert!(none.is_empty());
    sender.delta.set_work(dir.path().join("work"));
    let (_b, told) = ask_with(&asker, &ticket, root, 16 << 20, Duration::from_secs(5)).await;
    assert!(!told.is_empty(), "the failure was cached");
    assert_eq!(sender.delta.builds(), 2);
    asker.shutdown().await.unwrap();
    sender.serve.abort();
}
