//! More receives that rebuild from a local file: an insertion, content that shares nothing, a
//! folder, a candidate that is not a regular file, and a receive that is cut off and resumed.

use std::time::Duration;

use super::delta::{SIZE, bytes, delta_options, sender, stopped};
use super::*;

/// A megabyte put at the front moves every byte of the old file by that much, and the chunks after
/// the first boundary still match: the new file costs about the megabyte.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_insertion_at_the_front_costs_about_the_insertion() {
    let dir = tempfile::tempdir().unwrap();
    let old = bytes(SIZE, 11);
    let mut new = bytes(1 << 20, 12);
    new.extend_from_slice(&old);
    let from = dir.path().join("old.bin");
    fs::write(&from, &old).unwrap();
    let sender = sender(&dir, &new).await;
    let out = dir.path().join("received");
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let received = super::receive(&sender.text, &delta_options(&out, &from), &tx)
        .await
        .unwrap();
    assert_eq!(fs::read(out.join("source.bin")).unwrap(), new);
    let report = received.delta.unwrap();
    assert!(
        report.network_bytes < (1 << 20) + (1 << 18),
        "{} bytes came from the network",
        report.network_bytes
    );
    assert_eq!(report.local_bytes + report.network_bytes, new.len() as u64);
    stopped(sender).await;
}

/// Content that shares nothing with the candidate is fetched plainly: no assembly is made, and the
/// receive reports every byte as fetched.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unrelated_content_is_fetched_plainly() {
    let dir = tempfile::tempdir().unwrap();
    let new = bytes(SIZE, 13);
    let from = dir.path().join("other.bin");
    fs::write(&from, bytes(SIZE, 14)).unwrap();
    let sender = sender(&dir, &new).await;
    let out = dir.path().join("received");
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let received = super::receive(&sender.text, &delta_options(&out, &from), &tx)
        .await
        .unwrap();
    assert_eq!(fs::read(out.join("source.bin")).unwrap(), new);
    let report = received.delta.unwrap();
    assert_eq!((report.local_bytes, report.network_bytes), (0, SIZE as u64));
    assert_eq!(sender.delta.served(), 1);
    assert!(
        sender.delta.needs().is_empty(),
        "ranges were sent for a plain fetch"
    );
    stopped(sender).await;
}

/// A folder is matched by the path the ticket names: the candidate for `tree/big.bin` is
/// `<from>/tree/big.bin`, and a file with no counterpart is fetched whole.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_folder_is_matched_by_manifest_path() {
    let dir = tempfile::tempdir().unwrap();
    let new = bytes(SIZE, 15);
    let mut old = new.clone();
    old[2 << 20] ^= 1;
    let source = dir.path().join("tree");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("big.bin"), &new).unwrap();
    fs::write(source.join("fresh.bin"), bytes(SIZE, 16)).unwrap();
    let (outgoing, text) = prepared_paths(&dir, std::slice::from_ref(&source)).await;
    let serving = tokio::spawn(async move { outgoing.serve(Keep::One, &no_events()).await });
    let candidates = dir.path().join("have");
    fs::create_dir_all(candidates.join("tree")).unwrap();
    fs::write(candidates.join("tree").join("big.bin"), &old).unwrap();
    let out = dir.path().join("received");
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let received = super::receive(&text, &delta_options(&out, &candidates), &tx)
        .await
        .unwrap();
    assert_eq!(fs::read(out.join("tree").join("big.bin")).unwrap(), new);
    let report = received.delta.unwrap();
    assert!(report.local_bytes > SIZE as u64 * 3 / 4, "{report:?}");
    assert!(
        report.network_bytes >= SIZE as u64,
        "the new file came whole"
    );
    let end = tokio::time::timeout(Duration::from_secs(20), serving)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(end, StopEnd::Downloaded);
}

/// A candidate that is a link, or a folder where a file is expected, is not used and the file
/// arrives plainly.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_candidate_that_is_not_a_regular_file_is_not_used() {
    let dir = tempfile::tempdir().unwrap();
    let new = bytes(SIZE, 17);
    let tree = dir.path().join("tree");
    fs::create_dir_all(tree.join("source.bin")).unwrap();
    #[allow(unused_mut)]
    let mut candidates = vec![tree];
    #[cfg(unix)]
    {
        let real = dir.path().join("real.bin");
        fs::write(&real, &new).unwrap();
        let link = dir.path().join("link.bin");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        candidates.push(link);
    }
    for from in candidates {
        let dir = tempfile::tempdir_in(dir.path()).unwrap();
        let sender = sender(&dir, &new).await;
        let out = dir.path().join("received");
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let received = super::receive(&sender.text, &delta_options(&out, &from), &tx)
            .await
            .unwrap();
        assert_eq!(fs::read(out.join("source.bin")).unwrap(), new);
        let report = received.delta.unwrap();
        assert_eq!((report.local_bytes, report.network_bytes), (0, SIZE as u64));
        assert_eq!(sender.delta.served(), 0, "{from:?} was used");
        stopped(sender).await;
    }
}

/// A receive that is cut off, in the middle of a delta and after one, is resumed by the next run,
/// which completes the file.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cut_off_delta_receive_is_resumed() {
    let dir = tempfile::tempdir().unwrap();
    let new = bytes(SIZE, 18);
    let mut old = new.clone();
    for byte in &mut old[1 << 20..(1 << 20) + 300_000] {
        *byte ^= 0x33;
    }
    let from = dir.path().join("old.bin");
    fs::write(&from, &old).unwrap();
    let sender = sender(&dir, &new).await;
    let out = dir.path().join("received");

    // Cut off by a full disk once the content is in the store.
    let mut limited = delta_options(&out, &from);
    limited.write_limit = Some(256 * 1024);
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    super::receive(&sender.text, &limited, &tx)
        .await
        .unwrap_err();

    let received = super::receive(&sender.text, &delta_options(&out, &from), &tx)
        .await
        .unwrap();
    assert_eq!(fs::read(out.join("source.bin")).unwrap(), new);
    assert_eq!(received.bytes, new.len() as u64);
    stopped(sender).await;
}
