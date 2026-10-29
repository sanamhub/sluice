//! Taking part of a folder ticket: what a selection writes, what it refuses, which content it asks
//! for, and how it meets a partial store.

use super::*;
use crate::engine::Selection;

/// The options of a receive that takes only the entries the patterns match.
fn selecting(out: &Path, patterns: &[&str]) -> super::ReceiveOptions {
    let mut options = options(out);
    let patterns: Vec<String> = patterns
        .iter()
        .map(|pattern| (*pattern).to_string())
        .collect();
    options.include = Some(Selection::new(&patterns).unwrap());
    options
}

/// The ticket's store path under `out`, and the root its record is keyed by.
fn store_of(out: &Path, ticket: &str) -> (String, std::path::PathBuf) {
    let decoded = Ticket::decode(ticket).unwrap();
    let Body::Static { root, .. } = decoded.body() else {
        panic!("a live ticket");
    };
    let owner: String = root.iter().map(|byte| format!("{byte:02x}")).collect();
    let partial = crate::engine::storage::partial_dir(out, &owner);
    (owner, partial)
}

/// The id of the record the home holds for the store, which a new claim changes.
fn record_id(partial: &Path, owner: &str) -> String {
    match crate::engine::storage::find_record(partial, owner).unwrap() {
        Some(record) => record.id,
        None => panic!(
            "no record for {} under {}; there: {:?}; marker: {:?}",
            partial.display(),
            owner,
            std::fs::read_dir(partial)
                .map(|listing| listing.flatten().map(|e| e.file_name()).collect::<Vec<_>>()),
            std::fs::read_to_string(partial.join("ticket"))
        ),
    }
}

fn tree(dir: &TempDir) -> std::path::PathBuf {
    let tree = dir.path().join("tree");
    fs::create_dir_all(tree.join("sub").join("deep")).unwrap();
    fs::write(tree.join("a.txt"), b"aaaa").unwrap();
    fs::write(tree.join("sub").join("b.txt"), b"bb").unwrap();
    fs::write(tree.join("sub").join("deep").join("c.txt"), b"ccc").unwrap();
    fs::write(tree.join("skip.log"), b"never taken").unwrap();
    tree
}

/// Two patterns take the files either of them names, with the folders above them, and the rest of
/// the ticket is not written at all. Only the taken files are counted in the final lines.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_patterns_take_their_files_and_the_folders_above_them() {
    let dir = tempfile::tempdir().unwrap();
    let source = tree(&dir);
    let (outgoing, text) = prepared_paths(&dir, std::slice::from_ref(&source)).await;
    let out = dir.path().join("out");
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let received = super::receive(&text, &selecting(&out, &["tree/*.txt", "tree/sub/**"]), &tx)
        .await
        .unwrap();

    assert_eq!(received.files, 3);
    assert_eq!(received.bytes, 9);
    assert_eq!(fs::read(out.join("tree").join("a.txt")).unwrap(), b"aaaa");
    assert_eq!(
        fs::read(out.join("tree").join("sub").join("b.txt")).unwrap(),
        b"bb"
    );
    assert_eq!(
        fs::read(out.join("tree").join("sub").join("deep").join("c.txt")).unwrap(),
        b"ccc"
    );
    assert!(
        !out.join("tree").join("skip.log").exists(),
        "a file no pattern names was written"
    );
    outgoing.provider.shutdown().await.unwrap();
}

/// A pattern that names a link takes the link, and the link rules are the ones a full receive
/// applies: a link needs `--links`, and is not made without it.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_pattern_that_names_a_link_takes_it_only_with_links() {
    let dir = tempfile::tempdir().unwrap();
    let source = tree(&dir);
    std::os::unix::fs::symlink("a.txt", source.join("link")).unwrap();
    let (outgoing, text) = prepared_paths(&dir, std::slice::from_ref(&source)).await;
    let out = dir.path().join("out");

    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut without = selecting(&out, &["tree/link"]);
    without.links = false;
    let received = super::receive(&text, &without, &tx).await.unwrap();
    assert_eq!(received.files, 0);
    assert!(
        !out.join("tree").join("link").exists(),
        "a link was made without --links"
    );

    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut with = selecting(&out, &["tree/link"]);
    with.links = true;
    with.force = true;
    super::receive(&text, &with, &tx).await.unwrap();
    assert_eq!(
        fs::read_link(out.join("tree").join("link")).unwrap(),
        Path::new("a.txt")
    );
    outgoing.provider.shutdown().await.unwrap();
}

/// A pattern that matches nothing is refused before the first byte is asked for, and leaves no
/// store behind.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_pattern_that_matches_nothing_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let source = tree(&dir);
    let (outgoing, text) = prepared_paths(&dir, std::slice::from_ref(&source)).await;
    let out = dir.path().join("out");
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let err = super::receive(&text, &selecting(&out, &["nothing/**"]), &tx)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Policy(_)), "{err}");
    assert!(err.to_string().contains("nothing/**"), "{err}");
    assert!(!partial_exists(&out), "a refused selection left a store");
    outgoing.provider.shutdown().await.unwrap();
}

/// A partial store is continued by a receive that takes the same entries, and a receive that takes
/// others starts a store of its own instead of resuming one that holds a different set.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_store_is_continued_by_the_same_selection_and_not_by_another() {
    let dir = tempfile::tempdir().unwrap();
    let source = tree(&dir);
    let (outgoing, text) = prepared_paths(&dir, std::slice::from_ref(&source)).await;
    let out = dir.path().join("out");
    let (owner, partial) = store_of(&out, &text);

    // A write that fails after the claim keeps the store, which is the state a resumed run meets.
    let mut limited = selecting(&out, &["tree/*.txt"]);
    limited.write_limit = Some(1);
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let err = super::receive(&text, &limited, &tx).await.unwrap_err();
    assert!(matches!(err, Error::Storage(_)), "{err}");
    assert!(partial.exists(), "the failed write left no store: {err}");
    let first = record_id(&partial, &owner);

    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let _ = super::receive(&text, &limited, &tx).await.unwrap_err();
    assert_eq!(
        record_id(&partial, &owner),
        first,
        "a receive with the same selection claimed a new store"
    );

    // The other selection takes another file, so it cannot continue a store holding the first set.
    let mut other = selecting(&out, &["tree/sub/**"]);
    other.write_limit = Some(1);
    // The first runs made the top-level folder, which a run that starts its own store meets.
    other.force = true;
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let third = super::receive(&text, &other, &tx).await.unwrap_err();
    assert!(matches!(third, Error::Storage(_)), "{third}");
    assert_ne!(
        record_id(&partial, &owner),
        first,
        "a receive with another selection continued the store"
    );
    outgoing.provider.shutdown().await.unwrap();
}

/// A ticket whose unselected file has no content on the sender still arrives: only the taken files
/// are asked for, and the run would fail if it asked for the other one. Without a selection the
/// same ticket fails, which is what makes the first success mean something.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_content_of_an_unselected_file_is_not_fetched() {
    let dir = tempfile::tempdir().unwrap();
    let (outgoing, text) = provider_without_one_blob(&dir).await;
    let out = dir.path().join("out");
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let received = super::receive(&text, &selecting(&out, &["tree/a.txt"]), &tx)
        .await
        .unwrap();
    assert_eq!(received.files, 1);
    assert_eq!(received.bytes, 5);
    assert_eq!(fs::read(out.join("tree").join("a.txt")).unwrap(), b"taken");
    assert!(!out.join("tree").join("b.txt").exists());

    // The control: the same ticket without a selection asks for the file nobody serves and fails.
    let other = dir.path().join("all");
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut whole = options(&other);
    whole.max_network_failures = 1;
    let err = super::receive(&text, &whole, &tx).await.unwrap_err();
    assert!(matches!(err, Error::Network(_) | Error::Verify(_)), "{err}");
    outgoing.shutdown().await.unwrap();
}

/// A provider of a two-file manifest that serves `tree/a.txt` and has no bytes for `tree/b.txt`,
/// whose root no policy allows. A receive that asks for the second file fails.
async fn provider_without_one_blob(dir: &TempDir) -> (Provider, String) {
    let home = Home::open(&dir.path().join("two-home")).unwrap();
    let store = FsStore::load(dir.path().join("two-store")).await.unwrap();
    let taken = b"taken";
    let hash = store.add_slice(taken).await.unwrap().hash;
    let entry = |name: &str, size: u64, root: [u8; 32]| Entry {
        path: ManifestPath::parse(name).unwrap(),
        kind: Kind::File,
        size,
        mtime_ms: 0,
        mode: 0,
        loc: Loc::Blob { root },
        link_target: None,
    };
    let manifest = Manifest::new(
        vec![
            Entry {
                path: ManifestPath::parse("tree").unwrap(),
                kind: Kind::Dir,
                size: 0,
                mtime_ms: 0,
                mode: 0,
                loc: Loc::None,
                link_target: None,
            },
            entry("tree/a.txt", taken.len() as u64, *hash.as_bytes()),
            entry("tree/b.txt", 2048, [9; 32]),
        ],
        vec![],
    )
    .unwrap();
    store.blobs().add_bytes(manifest.encode()).await.unwrap();
    store
        .blobs()
        .add_bytes(manifest.sequence_bytes())
        .await
        .unwrap();
    let mut allowed: BTreeSet<[u8; 32]> = manifest.hash_sequence().into_iter().collect();
    allowed.insert(manifest.sequence_root());
    let provider = Provider::start(
        &home,
        store,
        allowed,
        RelayModeChoice::Disabled,
        super::Congestion::Cubic,
        None,
        None,
    )
    .await
    .unwrap();
    let addr = provider.addr();
    let ticket = Ticket::new(
        *addr.id.as_bytes(),
        addr.ip_addrs().copied().collect(),
        None,
        None,
        Body::Static {
            root: manifest.sequence_root(),
            size: 5 + 2048,
            format: Format::HashSequence,
        },
    )
    .unwrap();
    (provider, ticket.encode())
}
