//! The receipt file a receive writes: where it may go, and what a failure to write it costs. Each
//! case is one real transfer on loopback.

use std::time::Duration;

use super::receipts::{BODY, with_receipt};
use super::*;
use crate::engine::Event;

/// A receipt path that is `--out` or inside it is refused, whatever the manifest holds and whether
/// or not `--out` exists, and the refusal comes before a folder is made.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_receipt_path_inside_out_is_refused_before_a_folder_is_made() {
    for spelling in [
        "source.bin",
        "SOURCE.BIN",
        "./source.bin",
        "other/name.txt",
        "",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let (_outgoing, text) = prepared(&dir, BODY).await;
        let out = dir.path().join("received");
        let receipt = if spelling.is_empty() {
            out.clone()
        } else {
            out.join(spelling)
        };
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let error = super::receive(&text, &with_receipt(&out, &receipt), &tx)
            .await
            .expect_err(spelling);
        assert_eq!(error.kind(), crate::ErrorKind::Policy, "{spelling}");
        assert!(error.to_string().contains("outside --out"), "{error}");
        assert!(!out.exists(), "{spelling}: the refusal left --out behind");
    }
}

/// Links a folder to `out`: a symbolic link on Unix and a junction on Windows.
fn alias_of(out: &Path, alias: &Path) {
    #[cfg(unix)]
    std::os::unix::fs::symlink(out, alias).unwrap();
    #[cfg(windows)]
    {
        let made = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(alias)
            .arg(out)
            .output()
            .unwrap();
        assert!(made.status.success(), "mklink /J failed");
    }
}

/// `--force` with a path that reaches `--out` through a link never replaces a received file: the
/// receipt path is refused and the file keeps its bytes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn force_with_an_aliased_path_never_replaces_a_received_file() {
    let dir = tempfile::tempdir().unwrap();
    let (_outgoing, text) = prepared(&dir, BODY).await;
    let out = dir.path().join("received");
    fs::create_dir(&out).unwrap();
    fs::write(out.join("source.bin"), b"received").unwrap();
    let alias = dir.path().join("alias");
    alias_of(&out, &alias);
    let mut options = with_receipt(&out, &alias.join("source.bin"));
    options.force = true;
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let error = super::receive(&text, &options, &tx).await.unwrap_err();
    assert_eq!(error.kind(), crate::ErrorKind::Policy);
    assert_eq!(fs::read(out.join("source.bin")).unwrap(), b"received");
    assert_eq!(fs::read_dir(&out).unwrap().count(), 1);
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
            Event::Warning { message } if message.contains("the receipt arrived but was not written")
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
