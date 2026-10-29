//! A store of another transfer, in the name this receive would use: refused without being read,
//! changed or removed.

use super::*;

/// A store has to be this transfer's, not merely this home's: a record whose id a marker carries but
/// whose root is another ticket's names a store another receive is building. Refusing it leaves the
/// folder, its marker and the record exactly as they were, and starts no new store beside them.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_store_of_another_transfer_is_refused_without_removing_it() {
    let dir = tempfile::tempdir().unwrap();

    let one = dir.path().join("one.bin");
    fs::write(&one, vec![1u8; 64 * 1024]).unwrap();
    let two = dir.path().join("two.bin");
    fs::write(&two, vec![2u8; 64 * 1024]).unwrap();

    let (outgoing, text_one) =
        prepared_paths_in(&dir, "home-one", std::slice::from_ref(&one)).await;

    let (second, text_two) = prepared_paths_in(&dir, "home-two", std::slice::from_ref(&two)).await;

    let owners = [owner_of(&text_one), owner_of(&text_two)];

    let out = dir.path().join("out");
    fs::create_dir_all(&out).unwrap();

    let partial = crate::engine::storage::partial_dir(&out, &owners[1]);
    crate::engine::storage::claim(&partial).unwrap();

    let names = std::collections::BTreeSet::new();
    // The marker of the first transfer's store, under the name the second transfer would use, and a
    // record for the second transfer that carries the same id.
    crate::engine::storage::remember(&partial, &owners[0], &names, false, "a-record", "all")
        .unwrap();
    crate::engine::storage::remember_as(&partial, &owners[1], "a-record").unwrap();
    let marker = fs::read_to_string(partial.join(crate::engine::storage::MARKER)).unwrap();

    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let err = super::receive(&text_two, &options(&out), &tx)
        .await
        .unwrap_err();

    assert!(matches!(err, Error::Policy(_)), "{err}");
    assert!(err.to_string().contains(".sluice-partial-"), "{err}");
    assert!(
        partial.is_dir(),
        "the store of another transfer was removed"
    );
    assert_eq!(
        fs::read_to_string(partial.join(crate::engine::storage::MARKER)).unwrap(),
        marker
    );
    assert!(!out.join("two.bin").exists());

    // This transfer still has its own store to make beside that one.
    super::receive(&text_one, &options(&out), &tx)
        .await
        .unwrap();
    assert_eq!(fs::read(out.join("one.bin")).unwrap().len(), 64 * 1024);
    outgoing.provider.shutdown().await.unwrap();
    second.provider.shutdown().await.unwrap();
}
