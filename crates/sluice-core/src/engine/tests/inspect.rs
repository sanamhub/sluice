//! What a ticket offers, read from its manifest alone: the names, the counts and the size, with no
//! content fetched and nothing written anywhere.

use super::*;
use crate::engine::{Offering, inspect};

/// A one-file ticket offers that file, its size, and no name it does not have.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_one_file_ticket_offers_one_file() {
    let dir = tempfile::tempdir().unwrap();
    let body = vec![3u8; 4096];
    let (outgoing, text) = prepared(&dir, &body).await;
    let offering = inspect(&text, Limits::default()).await.unwrap();
    let Offering::Static(offered) = offering else {
        panic!("a file ticket was read as a live stream");
    };
    assert_eq!(offered.files, 1);
    assert_eq!(offered.folders, 0);
    assert_eq!(offered.links, 0);
    assert_eq!(offered.bytes, body.len() as u64);
    assert_eq!(offered.entries.len(), 1);
    assert_eq!(offered.entries[0].path, "source.bin");
    assert_eq!(offered.entries[0].kind, Kind::File);
    assert_eq!(offered.entries[0].size, body.len() as u64);
    // Reading a ticket writes nothing: the output folder was never made.
    assert!(!dir.path().join("out").exists());
    outgoing.provider.shutdown().await.unwrap();
}

/// A folder's ticket offers every folder and file under it, counted by kind, so the tree a user sees
/// is the tree that will be written.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_folder_ticket_offers_its_folders_and_files() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("tree");
    fs::create_dir_all(source.join("sub")).unwrap();
    fs::write(source.join("a.txt"), b"aaaa").unwrap();
    fs::write(source.join("sub").join("b.txt"), b"bb").unwrap();
    let (outgoing, text) = prepared_paths(&dir, std::slice::from_ref(&source)).await;
    let offering = inspect(&text, Limits::default()).await.unwrap();
    let Offering::Static(offered) = offering else {
        panic!("a folder ticket was read as a live stream");
    };
    assert_eq!(offered.files, 2);
    assert_eq!(offered.folders, 2);
    assert_eq!(offered.bytes, 6);
    let names: Vec<&str> = offered
        .entries
        .iter()
        .map(|entry| entry.path.as_str())
        .collect();
    assert_eq!(names, ["tree", "tree/a.txt", "tree/sub", "tree/sub/b.txt"]);
    outgoing.provider.shutdown().await.unwrap();
}

/// A ticket that promises a size its manifest does not add up to is refused as a receive refuses
/// it, before anything is shown: a name and a size a receive would never write must not be read as
/// if it would.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_ticket_whose_size_the_manifest_does_not_match_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let body = vec![9u8; 2048];
    let (provider, _hash, consistent) = provider_of_lying_manifest(&dir, &body, 2048, 0).await;
    let addr = provider.addr();
    let Body::Static { root, format, .. } = consistent.body() else {
        panic!("a live ticket");
    };
    let promised = 2047;
    let lying = Ticket::new(
        *addr.id.as_bytes(),
        addr.ip_addrs().copied().collect(),
        None,
        None,
        Body::Static {
            root: *root,
            size: promised,
            format: *format,
        },
    )
    .unwrap();

    let inspected = inspect(&lying.encode(), Limits::default())
        .await
        .unwrap_err();
    let out = dir.path().join("lying-out");
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let received = receive(&lying.encode(), &options(&out), &tx)
        .await
        .unwrap_err();
    assert_eq!(inspected.to_string(), received.to_string());
    assert!(inspected.to_string().contains("promises 2047"));
    provider.shutdown().await.unwrap();
}

/// A live ticket carries no manifest, so there is nothing to read: it is reported as a live stream
/// without reaching the sender at all.
#[tokio::test]
async fn a_live_ticket_is_a_live_stream() {
    let dir = tempfile::tempdir().unwrap();
    let home = Home::open(&dir.path().join("home")).unwrap();
    // A live ticket names a sender key, which is a key of the same kind as an endpoint's, and a
    // stream id, which is any 16 bytes.
    let key = home.identity().unwrap().endpoint_id();
    let ticket = Ticket::new(
        key,
        Vec::new(),
        None,
        None,
        Body::Live {
            stream_id: [1u8; 16],
            sender_key: key,
        },
    )
    .unwrap();
    let offering = inspect(&ticket.encode(), Limits::default()).await.unwrap();
    assert!(matches!(offering, Offering::Live));
}

/// A raw blob has no names, and its size is the whole of what its ticket says.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_raw_ticket_offers_its_size_and_no_name() {
    let dir = tempfile::tempdir().unwrap();
    let home = Home::open(&dir.path().join("home")).unwrap();
    let body = vec![4u8; 2 * 1024];
    let outgoing = crate::engine::prepare_send_stdin_from(&home, body.clone())
        .await
        .unwrap();
    let text = outgoing.ticket.encode();
    let offering = inspect(&text, Limits::default()).await.unwrap();
    let Offering::Static(offered) = offering else {
        panic!("a raw ticket was read as a live stream");
    };
    assert_eq!(offered.files, 1);
    assert_eq!(offered.folders, 0);
    assert_eq!(offered.bytes, body.len() as u64);
    assert!(offered.entries.is_empty(), "a raw blob has no name");
    outgoing.provider.shutdown().await.unwrap();
}
