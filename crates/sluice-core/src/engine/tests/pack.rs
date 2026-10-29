//! Taking part of a ticket whose small files share a pack: the whole pack is fetched, and the room
//! the run needs and the progress total are the pack's bytes rather than the bytes it writes.

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

/// A provider of a manifest whose two small files share one pack, and which also lists a big file
/// that is a blob of its own. A selection of one small file therefore fetches the whole pack, and a
/// selection of only the big file would fetch less than the ticket promises. No content is served
/// for the big file: no selection here asks for it.
async fn provider_with_a_pack(dir: &TempDir) -> (Provider, String) {
    let home = Home::open(&dir.path().join("pack-home")).unwrap();
    let store = FsStore::load(dir.path().join("pack-store")).await.unwrap();
    let packed = b"takenuntaken";
    let pack = store.add_slice(packed).await.unwrap().hash;
    let entry = |name: &str, size: u64, loc: Loc| Entry {
        path: ManifestPath::parse(name).unwrap(),
        kind: Kind::File,
        size,
        mtime_ms: 0,
        mode: 0,
        loc,
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
            entry(
                "tree/a.txt",
                5,
                Loc::Packed {
                    pack_index: 0,
                    offset: 0,
                    len: 5,
                },
            ),
            entry(
                "tree/b.txt",
                7,
                Loc::Packed {
                    pack_index: 0,
                    offset: 5,
                    len: 7,
                },
            ),
            entry("tree/big.bin", 20, Loc::Blob { root: [9; 32] }),
        ],
        vec![PackRef {
            root: *pack.as_bytes(),
            size: packed.len() as u64,
        }],
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
            size: 5 + 7 + 20,
            format: Format::HashSequence,
        },
    )
    .unwrap();
    (provider, ticket.encode())
}

/// The room a run needs is the bytes it fetches: the pack's twelve, not the five it writes and not
/// the ticket's thirty-two. Five bytes free refuse it, and twelve are enough. The planned line
/// counts the file it takes and says what the pack moves, and progress ends at the pack's whole
/// size.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_selection_counts_the_whole_pack_it_fetches() {
    let dir = tempfile::tempdir().unwrap();
    let (provider, text) = provider_with_a_pack(&dir).await;

    let tight_out = dir.path().join("tight");
    let mut tight = selecting(&tight_out, &["tree/a.txt"]);
    tight.free_bytes = Some(5);
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let err = super::receive(&text, &tight, &tx).await.unwrap_err();
    assert!(matches!(err, Error::Storage(_)), "{err}");
    assert!(err.to_string().contains("needs about"), "{err}");
    assert!(
        !tight_out.join("tree").exists(),
        "a refused selection wrote a file"
    );

    let out = dir.path().join("out");
    fs::create_dir(&out).unwrap();
    let needed =
        crate::engine::storage::needed_bytes(12, crate::engine::storage::supports_links(&out));
    let mut enough = selecting(&out, &["tree/a.txt"]);
    enough.free_bytes = Some(needed);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let received = super::receive(&text, &enough, &tx).await.unwrap();
    assert_eq!(received.files, 1);
    assert_eq!(received.bytes, 5);
    assert_eq!(fs::read(out.join("tree").join("a.txt")).unwrap(), b"taken");
    assert!(!out.join("tree").join("b.txt").exists());

    let mut planned = None;
    let mut progress = None;
    while let Ok(event) = rx.try_recv() {
        match event {
            Event::Planned { bytes, fetched, .. } => planned = Some((bytes, fetched)),
            Event::Progress { bytes, total } => progress = Some((bytes, total)),
            _ => {}
        }
    }
    assert_eq!(planned, Some((5, Some(12))), "the planned line");
    assert_eq!(progress, Some((12, 12)), "the progress total is the pack");
    provider.shutdown().await.unwrap();
}
