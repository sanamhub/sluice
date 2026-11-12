//! The receipt file a receive writes: where it may go, and what a failure to write it costs. Each
//! case is one real transfer on loopback.

use std::time::Duration;

use super::receipts::{BODY, with_receipt};
use super::*;
use crate::engine::Event;

/// A receipt path that is a file the transfer writes into `--out` is refused before any content
/// moves, in the spelling the manifest uses and in another case, and nothing is left in `--out`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_receipt_path_the_transfer_writes_is_refused_before_anything_arrives() {
    for spelling in ["source.bin", "SOURCE.BIN", "./source.bin"] {
        let dir = tempfile::tempdir().unwrap();
        let (outgoing, text) = prepared(&dir, BODY).await;
        let serve = tokio::spawn(async move { outgoing.serve(Keep::One, &no_events()).await });
        let out = dir.path().join("received");
        fs::create_dir(&out).unwrap();
        let receipt = out.join(spelling);
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let error = super::receive(&text, &with_receipt(&out, &receipt), &tx)
            .await
            .expect_err(spelling);
        assert_eq!(error.kind(), crate::ErrorKind::Policy, "{spelling}");
        assert!(error.to_string().contains("outside --out"), "{error}");
        let left: Vec<_> = fs::read_dir(&out)
            .map(|entries| entries.map(|e| e.unwrap().file_name()).collect())
            .unwrap_or_default();
        assert!(left.is_empty(), "{spelling}: {left:?} was left in --out");
        serve.abort();
        let _ = serve.await;
    }
}

/// A receipt that arrived but cannot be written is a warning: the content is in place and the
/// receive succeeds. The file that took the name before the write is not replaced, and no temporary
/// file is left beside it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_receipt_that_cannot_be_written_is_a_warning() {
    let dir = tempfile::tempdir().unwrap();
    let (outgoing, text) = prepared(&dir, BODY).await;
    let serve = tokio::spawn(async move { outgoing.serve(Keep::One, &no_events()).await });
    let out = dir.path().join("received");
    let kept = dir.path().join("mine");
    fs::create_dir(&kept).unwrap();
    let receipt = kept.join("receipt.txt");
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let options = with_receipt(&out, &receipt);
    let receive = tokio::spawn(async move { super::receive(&text, &options, &tx).await });
    // The first event comes before the content is fetched, so the name is taken after the check
    // that it is free and before the write that needs it.
    let first = rx.recv().await.expect("the receive reported nothing");
    fs::write(&receipt, "kept").unwrap();
    let mut events = vec![first];
    while let Some(event) = rx.recv().await {
        events.push(event);
    }
    let received = receive
        .await
        .unwrap()
        .expect("the receive failed over a receipt");
    assert_eq!(received.bytes, BODY.len() as u64);
    assert_eq!(fs::read(out.join("source.bin")).unwrap(), BODY);
    assert!(
        events.iter().any(|event| matches!(
            event,
            Event::Warning { message } if message.contains("was not written")
        )),
        "no warning said the receipt was not written"
    );
    assert_eq!(fs::read_to_string(&receipt).unwrap(), "kept");
    let names: Vec<_> = fs::read_dir(&kept)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names, ["receipt.txt"]);
    let end = tokio::time::timeout(Duration::from_secs(15), serve)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(end, super::StopEnd::Downloaded);
}
