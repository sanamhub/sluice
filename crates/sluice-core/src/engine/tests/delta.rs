//! A receive that rebuilds a file from an older version of it: what it saves, what it leaves alone,
//! and that the result is the file the sender named. Each case is one real transfer on loopback.

use std::sync::Arc;
use std::time::Duration;

use super::*;
use crate::engine::delta::Delta;

/// `len` bytes from a fixed seed, so a failure is the same failure on the next run.
pub(super) fn bytes(len: usize, seed: u64) -> Vec<u8> {
    let mut state = seed | 1;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 3) as u8
        })
        .collect()
}

pub(super) const SIZE: usize = 4 << 20;

/// A sender of `new` named `source.bin`, serving one download, and its delta state.
pub(super) struct Sender {
    pub(super) text: String,
    pub(super) serve: tokio::task::JoinHandle<Result<StopEnd, crate::Error>>,
    pub(super) delta: Arc<Delta>,
}

pub(super) async fn sender(dir: &TempDir, new: &[u8]) -> Sender {
    let (outgoing, text) = prepared(dir, new).await;
    let delta = Arc::clone(
        outgoing
            .provider
            .delta()
            .expect("a fixed send serves delta"),
    );
    let serve = tokio::spawn(async move { outgoing.serve(Keep::One, &no_events()).await });
    Sender { text, serve, delta }
}

pub(super) fn delta_options(out: &Path, from: &Path) -> ReceiveOptions {
    let mut options = options(out);
    options.delta_from = Some(from.to_path_buf());
    options
}

pub(super) async fn stopped(sender: Sender) {
    let end = tokio::time::timeout(Duration::from_secs(20), sender.serve)
        .await
        .expect("the sender stopped")
        .unwrap()
        .unwrap();
    assert_eq!(end, StopEnd::Downloaded);
}

/// An older version that differs from the new one in one place is the source of the rest: the
/// result is the new file, most of it came from the old one, and the old one is as it was.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_changed_file_is_rebuilt_from_the_old_one() {
    let dir = tempfile::tempdir().unwrap();
    let new = bytes(SIZE, 1);
    let mut old = new.clone();
    for byte in &mut old[1 << 20..(1 << 20) + 65_536] {
        *byte ^= 0x5a;
    }
    let from = dir.path().join("old.bin");
    fs::write(&from, &old).unwrap();
    let modified = fs::metadata(&from).unwrap().modified().unwrap();
    let sender = sender(&dir, &new).await;
    let out = dir.path().join("received");
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let received = super::receive(&sender.text, &delta_options(&out, &from), &tx)
        .await
        .unwrap();
    assert_eq!(fs::read(out.join("source.bin")).unwrap(), new);
    let report = received.delta.expect("a delta receive reports its sources");
    assert_eq!(report.local_bytes + report.network_bytes, SIZE as u64);
    assert!(
        report.network_bytes < SIZE as u64 / 8,
        "{} of {SIZE} bytes came from the network",
        report.network_bytes
    );
    assert_eq!(sender.delta.served(), 1);
    // The requester sent one message after the root, the ranges it still needed, and they lie
    // around the 64 KiB that changed and nowhere else.
    let needs = sender.delta.needs();
    assert_eq!(needs.len(), 1);
    assert_eq!(needs[0].root(), blake3::hash(&new).as_bytes());
    let (first, last) = (needs[0].ranges()[0], *needs[0].ranges().last().unwrap());
    assert!(first.start() >= (1 << 20) - 65_536 && last.end() <= (1 << 20) + 2 * 65_536 + 65_536);
    assert_eq!(fs::read(&from).unwrap(), old, "the candidate was written");
    assert_eq!(fs::metadata(&from).unwrap().modified().unwrap(), modified);
    stopped(sender).await;
}

/// Without the flag nothing is asked: the sender writes no list and the receive reports nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_receive_without_the_flag_asks_for_no_signatures() {
    let dir = tempfile::tempdir().unwrap();
    let new = bytes(SIZE, 2);
    let sender = sender(&dir, &new).await;
    let out = dir.path().join("received");
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let received = super::receive(&sender.text, &options(&out), &tx)
        .await
        .unwrap();
    assert_eq!(received.delta, None);
    assert_eq!(sender.delta.served(), 0);
    assert_eq!(fs::read(out.join("source.bin")).unwrap(), new);
    stopped(sender).await;
}
