//! The sender's list builds: one build for every requester of a root, a build that stops when its
//! requesters have gone, a wait that ends in a plain fetch, and a third build refused while two
//! hold the permits.

use std::time::Duration;

use iroh_blobs::{Hash, protocol::GetRequest, store::fs::FsStore};

use super::delta::{SIZE, bytes, delta_options, sender, stopped};
use super::*;
use crate::engine::hello::{CAP_DELTA, greet_stream_with};

/// Asks for the list of `root` on a connection of its own, and returns the connection with what the
/// sender wrote back after `wait`. Dropping the connection is leaving.
async fn ask(
    asker: &Receiver,
    ticket: &Ticket,
    root: [u8; 32],
    wait: Duration,
) -> (iroh::endpoint::Connection, Vec<u8>) {
    let conn = asker
        .connect_alpn(ticket, super::ALPN, no_events())
        .await
        .unwrap();
    let ours = super::Hello::ours().with(CAP_DELTA);
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

/// Fetches every root over one content connection, which is what admits the endpoint for them.
async fn admit_all(
    dir: &TempDir,
    asker: &Receiver,
    ticket: &Ticket,
    roots: &[[u8; 32]],
) -> FsStore {
    let conn = asker.connect_static(ticket, no_events()).await.unwrap();
    let store = FsStore::load(dir.path().join("asker-store")).await.unwrap();
    for root in roots {
        store
            .remote()
            .execute_get(conn.clone(), GetRequest::blob(Hash::from(*root)))
            .complete()
            .await
            .unwrap();
    }
    store
}

/// A sender that serves until it is stopped, for a test that waits on builds with no receiver
/// fetching: a one-download sender would end on its grace period once the roots were delivered.
async fn lasting_sender(dir: &TempDir, new: &[u8]) -> super::delta::Sender {
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

/// A receiver whose wait for the list ends before the sender has built it fetches plainly, the file
/// verifies, and the sender's permit is free once the receiver has gone.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_slow_list_ends_in_a_plain_fetch_and_frees_the_permit() {
    let dir = tempfile::tempdir().unwrap();
    let new = bytes(SIZE, 21);
    let from = dir.path().join("old.bin");
    fs::write(&from, &new).unwrap();
    let sender = sender(&dir, &new).await;
    sender.delta.delay_builds(Duration::from_secs(30));
    let out = dir.path().join("received");
    let mut options = delta_options(&out, &from);
    options.delta_wait = Some(Duration::from_secs(1));
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let received = super::receive(&sender.text, &options, &tx).await.unwrap();
    assert_eq!(fs::read(out.join("source.bin")).unwrap(), new);
    let report = received.delta.unwrap();
    assert_eq!((report.local_bytes, report.network_bytes), (0, SIZE as u64));
    // The build was for a receiver that has left, so it stops and gives its permit back.
    let delta = std::sync::Arc::clone(&sender.delta);
    eventually("the permit was not given back", move || {
        delta.free_permits() == 2
    })
    .await;
    stopped(sender).await;
}

/// A requester that asks and closes its connection frees the permit its build held.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_requester_that_leaves_frees_the_build_it_waited_for() {
    let dir = tempfile::tempdir().unwrap();
    let new = bytes(SIZE, 22);
    let root = *blake3::hash(&new).as_bytes();
    let sender = lasting_sender(&dir, &new).await;
    sender.delta.delay_builds(Duration::from_secs(30));
    let ticket = Ticket::decode(&sender.text).unwrap();
    let asker = Receiver::open(RelayModeChoice::Disabled, super::Congestion::Cubic)
        .await
        .unwrap();
    let _store = admit_all(&dir, &asker, &ticket, &[root]).await;
    let (conn, got) = ask(&asker, &ticket, root, Duration::from_millis(500)).await;
    assert!(got.is_empty());
    assert_eq!(sender.delta.free_permits(), 1, "the build holds a permit");
    conn.close(0u32.into(), b"");
    let delta = std::sync::Arc::clone(&sender.delta);
    eventually("the permit was not given back", move || {
        delta.free_permits() == 2
    })
    .await;
    asker.shutdown().await.unwrap();
    sender.serve.abort();
}

/// Two requests for one root share one build, and both get the same list.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_requests_for_one_root_share_one_build() {
    let dir = tempfile::tempdir().unwrap();
    let new = bytes(SIZE, 23);
    let root = *blake3::hash(&new).as_bytes();
    let sender = lasting_sender(&dir, &new).await;
    sender.delta.delay_builds(Duration::from_millis(800));
    let ticket = Ticket::decode(&sender.text).unwrap();
    let asker = Receiver::open(RelayModeChoice::Disabled, super::Congestion::Cubic)
        .await
        .unwrap();
    let _store = admit_all(&dir, &asker, &ticket, &[root]).await;
    let (first, second) = tokio::join!(
        ask(&asker, &ticket, root, Duration::from_secs(5)),
        ask(&asker, &ticket, root, Duration::from_secs(5)),
    );
    assert!(!first.1.is_empty());
    assert_eq!(first.1, second.1);
    assert_eq!(sender.delta.builds(), 1, "a second build was started");
    asker.shutdown().await.unwrap();
    sender.serve.abort();
}

/// A third build, asked for while two hold both permits, is answered with nothing, and the receive
/// that asked falls back to a plain fetch that verifies.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_third_build_is_refused_while_two_hold_the_permits() {
    let dir = tempfile::tempdir().unwrap();
    let tree = dir.path().join("tree");
    fs::create_dir_all(&tree).unwrap();
    let files: Vec<Vec<u8>> = (0..3).map(|n| bytes(2 << 20, 31 + 2 * n)).collect();
    for (n, body) in files.iter().enumerate() {
        fs::write(tree.join(format!("f{n}.bin")), body).unwrap();
    }
    let roots: Vec<[u8; 32]> = files.iter().map(|b| *blake3::hash(b).as_bytes()).collect();
    let (outgoing, text) = prepared_paths(&dir, std::slice::from_ref(&tree)).await;
    let delta = std::sync::Arc::clone(outgoing.provider.delta().unwrap());
    // Until stopped: the sender must not end on its grace period while the test waits on builds.
    let serving =
        tokio::spawn(async move { outgoing.serve(Keep::UntilStopped, &no_events()).await });
    delta.delay_builds(Duration::from_secs(8));
    let ticket = Ticket::decode(&text).unwrap();
    let asker = Receiver::open(RelayModeChoice::Disabled, super::Congestion::Cubic)
        .await
        .unwrap();
    let _store = admit_all(&dir, &asker, &ticket, &roots).await;
    let held = [
        ask(&asker, &ticket, roots[0], Duration::from_millis(300)).await,
        ask(&asker, &ticket, roots[1], Duration::from_millis(300)).await,
    ];
    assert_eq!(delta.free_permits(), 0, "two builds hold both permits");
    let (_third, got) = ask(&asker, &ticket, roots[2], Duration::from_secs(2)).await;
    assert!(got.is_empty(), "a third build was answered");

    // A receive that has a candidate for the third file only is refused its list and fetches plainly.
    let have = dir.path().join("have");
    fs::create_dir_all(have.join("tree")).unwrap();
    fs::write(have.join("tree").join("f2.bin"), &files[2]).unwrap();
    let out = dir.path().join("received");
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let received = super::receive(&text, &delta_options(&out, &have), &tx)
        .await
        .unwrap();
    for (n, body) in files.iter().enumerate() {
        assert_eq!(
            fs::read(out.join("tree").join(format!("f{n}.bin"))).unwrap(),
            *body
        );
    }
    let report = received.delta.unwrap();
    assert_eq!(report.local_bytes, 0);
    assert_eq!(report.network_bytes, 3 * (2 << 20));
    drop(held);
    asker.shutdown().await.unwrap();
    serving.abort();
}
