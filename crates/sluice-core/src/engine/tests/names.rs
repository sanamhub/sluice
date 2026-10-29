//! Two selections of one ticket into one output: the top-level names the first run made stay this
//! ticket's, so the second writes beside them without `--force`, while a file this ticket did not
//! make still clashes.

use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_second_selection_writes_beside_the_first_without_force() {
    let dir = tempfile::tempdir().unwrap();
    let source = tree(&dir);
    let (outgoing, text) = prepared_paths(&dir, std::slice::from_ref(&source)).await;
    let out = dir.path().join("out");

    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    super::receive(&text, &selecting(&out, &["tree/a.txt"]), &tx)
        .await
        .unwrap();
    assert_eq!(fs::read(out.join("tree").join("a.txt")).unwrap(), b"aaaa");

    // A file neither run made clashes, as it does today, and the refusal keeps the names the first
    // run recorded: removing the file makes the same selection work without `--force`.
    let hand = out.join("tree").join("sub").join("b.txt");
    fs::create_dir_all(hand.parent().unwrap()).unwrap();
    fs::write(&hand, b"mine").unwrap();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let err = super::receive(&text, &selecting(&out, &["tree/sub/**"]), &tx)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Policy(_)), "{err}");
    assert!(err.to_string().contains("already exists"), "{err}");
    fs::remove_file(&hand).unwrap();

    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    super::receive(&text, &selecting(&out, &["tree/sub/**"]), &tx)
        .await
        .unwrap();
    assert_eq!(
        fs::read(out.join("tree").join("sub").join("b.txt")).unwrap(),
        b"bb"
    );
    assert!(
        !out.join("tree").join("skip.log").exists(),
        "a file no selection named was written"
    );
    outgoing.provider.shutdown().await.unwrap();
}
