//! A receive that is cancelled while its storage work is still queued. The folder, the home's
//! record of it and its marker belong to one owner at a time, so a later receive of the same
//! output either finds a store it can resume or claims one of its own, and never has the folder or
//! the record taken from under it.

use std::time::Duration;

use super::*;

/// The ticket root in hex, which is part of the store's name, and the store's path under `out`.
fn store_of(out: &Path, ticket: &str) -> (String, std::path::PathBuf) {
    let decoded = Ticket::decode(ticket).unwrap();
    let Body::Static { root, .. } = decoded.body() else {
        panic!("a live ticket");
    };
    let owner: String = root.iter().map(|byte| format!("{byte:02x}")).collect();
    let partial = crate::engine::storage::partial_dir(out, &owner);
    (owner, partial)
}

/// The store a cancelled claim was making is the one the next run resumes: the folder, the record
/// and the marker are made by one blocking task, so a receive that is dropped while that task is
/// queued cannot stop between them and leave a folder that no record names, which the next run
/// would refuse as another transfer's.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[allow(clippy::await_holding_lock)] // the one test lock, held for the whole test
async fn a_claim_that_is_cancelled_leaves_a_store_the_next_run_resumes() {
    let _alone = crate::engine::storage::exclusive_removals();
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("one.bin");
    fs::write(&source, vec![5u8; 64 * 1024]).unwrap();
    let (outgoing, text) = prepared_paths(&dir, std::slice::from_ref(&source)).await;
    let out = dir.path().join("out");
    let (owner, partial) = store_of(&out, &text);

    // The claim removes a folder left under the store's name before it makes one, and the arm stops
    // it there: the folder, the record and the marker are all still ahead of it.
    let (reached, release) = crate::engine::storage::parked::arm(&partial);
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let waiting = text.clone();
    let opts = options(&out);
    let receiving = tokio::spawn(async move { super::receive(&waiting, &opts, &tx).await });
    tokio::task::spawn_blocking(move || reached.recv().unwrap())
        .await
        .unwrap();
    // Dropping the future is what an interrupt does. The blocking task is not a part of it, so it
    // finishes the claim: a folder with a record and a marker, or nothing at all.
    receiving.abort();
    assert!(receiving.await.unwrap_err().is_cancelled());
    release.send(()).unwrap();

    // The path is released only once that task is done, so waiting for the hold is waiting for the
    // store to be whole.
    let waited = partial.clone();
    tokio::task::spawn_blocking(move || crate::engine::storage::with_hold(&waited, || ()))
        .await
        .unwrap()
        .unwrap();
    let record = crate::engine::storage::find_record(&partial, &owner)
        .unwrap()
        .expect("the cancelled claim left no record");
    let found = crate::engine::storage::recorded(&partial)
        .unwrap()
        .expect("the cancelled claim left no marker");
    assert_eq!(found.owner.id.as_deref(), Some(record.id.as_str()));

    // The next run of the same ticket continues that store instead of refusing the folder it found.
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let received = super::receive(&text, &options(&out), &tx).await.unwrap();
    assert_eq!(received.files, 1);
    assert_eq!(fs::read(out.join("one.bin")).unwrap().len(), 64 * 1024);
    outgoing.provider.shutdown().await.unwrap();
}

/// A removal that outlives the receive which asked for it does not take the store a later receive
/// of the same output claims: the later claim waits for the removal to finish, and the store it
/// then makes keeps its folder and its record. The first receive is cancelled while its discard
/// waits on a held file, which is the state an interrupt leaves when a store's close is still being
/// tried.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::await_holding_lock)] // the one test lock, held for the whole test
async fn a_discard_that_outlives_its_receive_leaves_the_next_store_alone() {
    let _alone = crate::engine::storage::exclusive_removals();
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("one.bin");
    fs::write(&source, vec![7u8; 64 * 1024]).unwrap();
    let (outgoing, text) = prepared_paths(&dir, std::slice::from_ref(&source)).await;
    let out = dir.path().join("out");
    let (owner, partial) = store_of(&out, &text);

    let (tx, events) = tokio::sync::mpsc::unbounded_channel();
    let waiting = text.clone();
    let opts = options(&out);
    let receiving = tokio::spawn(async move { super::receive(&waiting, &opts, &tx).await });
    let release = arm_removal(events, partial.clone()).await;
    receiving.abort();
    let _ = receiving.await;
    assert!(
        out.join("one.bin").exists(),
        "the first receive wrote nothing"
    );

    // The output file goes, so the next receive has a file to write and can stop in the middle of
    // writing it, which keeps its store for the assertions below.
    fs::remove_file(out.join("one.bin")).unwrap();
    let mut limited = options(&out);
    limited.write_limit = Some(1);
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut second = tokio::spawn(async move { super::receive(&text, &limited, &tx).await });

    // While the removal is held, the second receive cannot get past its first look at the store.
    // The wait is the assertion: a receive that went on would finish here.
    assert!(
        tokio::time::timeout(Duration::from_millis(300), &mut second)
            .await
            .is_err(),
        "a receive went past a removal that still held the store"
    );
    release.send(()).unwrap();
    let error = tokio::time::timeout(Duration::from_secs(60), second)
        .await
        .expect("the second receive did not finish")
        .unwrap()
        .unwrap_err();
    assert!(matches!(error, Error::Storage(_)), "{error}");
    assert_the_store_is_the_second_runs(&partial, &owner);
    outgoing.provider.shutdown().await.unwrap();
}

/// The store the second receive claimed is its own: the folder, the record and the marker agree,
/// and the removal the first receive left behind did not take any of them.
fn assert_the_store_is_the_second_runs(partial: &Path, owner: &str) {
    assert!(
        partial.exists(),
        "the store the second receive claimed was removed"
    );
    let record = crate::engine::storage::find_record(partial, owner)
        .unwrap()
        .expect("the record of the second store was forgotten");
    let found = crate::engine::storage::recorded(partial)
        .unwrap()
        .expect("the marker of the second store was removed");
    assert_eq!(found.owner.id.as_deref(), Some(record.id.as_str()));
}

/// Arms the removal of `partial` once the receive reports that it started, which is after it has
/// claimed its store and before the removal that follows a finished transfer. Returns the sender
/// that lets that removal go on, once it has reached the arm.
async fn arm_removal(
    mut events: tokio::sync::mpsc::UnboundedReceiver<Event>,
    partial: std::path::PathBuf,
) -> std::sync::mpsc::Sender<()> {
    let watcher = tokio::spawn(async move {
        while let Some(event) = events.recv().await {
            if matches!(event, Event::Started { .. }) {
                return Some(crate::engine::storage::parked::arm(&partial));
            }
        }
        None
    });
    let (reached, release) = tokio::time::timeout(Duration::from_secs(60), watcher)
        .await
        .expect("the first receive reported nothing")
        .unwrap()
        .expect("the first receive did not report that it started");
    tokio::task::spawn_blocking(move || reached.recv().unwrap())
        .await
        .unwrap();
    release
}
