//! Live streams end to end: a file that grows while a receiver watches it, and a stream whose
//! records were damaged or reordered.

use std::sync::{Arc, Mutex};

use ed25519_dalek::SigningKey;
use iroh_blobs::store::fs::FsStore;
use sluice_live::SEGMENT_BYTES;

use super::*;
use crate::engine::{LiveLog, Received, prepare_live, receive_live};

/// A live sender whose file may still grow. The receive runs on the same runtime, as two
/// endpoints in one process on loopback.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_file_that_grows_is_followed_until_its_close() {
    let dir = tempfile::tempdir().unwrap();
    let home = Home::open(&dir.path().join("home")).unwrap();
    let source = dir.path().join("grow.bin");
    fs::write(&source, vec![1u8; 600 * 1024]).unwrap();
    let outgoing = prepare_live(
        &home,
        &source,
        RelayModeChoice::Disabled,
        Congestion::Cubic,
        None,
        None,
        Duration::from_millis(500),
    )
    .await
    .unwrap();
    let text = outgoing.ticket.encode();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let serving = tokio::spawn(async move { outgoing.serve(Keep::One, &tx).await });

    let out = dir.path().join("out.bin");
    let store_root = dir.path().join("live-store");
    fs::create_dir(&store_root).unwrap();
    let receiving = {
        let text = text.clone();
        let store_root = store_root.clone();
        let options = options(&out);
        tokio::spawn(async move {
            let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
            receive_live(&text, &options, &store_root, &tx).await
        })
    };
    // Append while the receiver is reading, which is the whole point of a live stream.
    tokio::time::sleep(Duration::from_millis(200)).await;
    {
        use std::io::Write;
        let mut file = fs::OpenOptions::new().append(true).open(&source).unwrap();
        file.write_all(&vec![2u8; 900 * 1024]).unwrap();
        file.sync_all().unwrap();
    }
    let received = receiving.await.unwrap().unwrap();
    let body = fs::read(&source).unwrap();
    assert_eq!(fs::read(&out).unwrap(), body);
    assert_eq!(received.bytes, body.len() as u64);
    assert_eq!(serving.await.unwrap().unwrap(), StopEnd::Downloaded);
}

/// A sender, its provider and its ticket, without a file to follow: the test pushes exactly the
/// frames it wants the receiver to meet.
struct Fixture {
    provider: Provider,
    ticket: Ticket,
    store: FsStore,
    log: Arc<LiveLog>,
    sender: sluice_live::Sender,
}

async fn fixture(dir: &TempDir, home: &Home) -> Fixture {
    let store = FsStore::load(dir.path().join("store")).await.unwrap();
    let key = SigningKey::from_bytes(&home.identity().unwrap().secret_seed());
    let sender_key = key.verifying_key().to_bytes();
    let stream_id = [3u8; 16];
    let sender = sluice_live::Sender::new(key, stream_id, 1 << 20, 0).unwrap();
    let log = Arc::new(LiveLog::new(
        sender.log_open(),
        Arc::new(Mutex::new(BTreeSet::new())),
    ));
    let mut provider = Provider::start_live(
        home,
        store.clone(),
        log.clone(),
        RelayModeChoice::Disabled,
        Congestion::Cubic,
        None,
        None,
    )
    .await
    .unwrap();
    // The fixture stands for a live send that has printed its ticket, so its mode is set.
    provider.set_limit(None);
    let addr = provider.addr();
    let ticket = Ticket::new(
        *addr.id.as_bytes(),
        addr.ip_addrs().copied().collect(),
        None,
        None,
        Body::Live {
            stream_id,
            sender_key,
        },
    )
    .unwrap();
    Fixture {
        provider,
        ticket,
        store,
        log,
        sender,
    }
}

/// Adds one full segment to the fixture and returns its frame length.
async fn seal(fixture: &mut Fixture, fill: u8) -> sluice_live::Frame {
    let bytes = vec![fill; 1 << 20];
    let tag = fixture.store.add_slice(&bytes).await.unwrap();
    fixture
        .sender
        .seal(*tag.hash.as_bytes(), bytes.len() as u32)
        .unwrap()
}

async fn receive(fixture: &Fixture, dir: &TempDir) -> Result<Received, Error> {
    let out = dir.path().join("out.bin");
    let store_root = dir.path().join("live-store");
    fs::create_dir(&store_root).unwrap();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    receive_live(&fixture.ticket.encode(), &options(&out), &store_root, &tx).await
}

/// A frame whose signature no sender produced is refused, and the receive stops.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_corrupted_frame_stops_the_receive() {
    let dir = tempfile::tempdir().unwrap();
    let home = Home::open(&dir.path().join("home")).unwrap();
    let mut fixture = fixture(&dir, &home).await;
    let frame = seal(&mut fixture, 7).await;
    fixture.log.push(frame);
    fixture.log.corrupt(0);
    let err = receive(&fixture, &dir).await.unwrap_err();
    assert!(matches!(err, Error::Verify(_)), "{err}");
    assert!(!dir.path().join("out.bin").exists());
    fixture.provider.shutdown().await.unwrap();
}

/// A frame that comes before the one the chain expects is refused, so a provider cannot make the
/// receiver write a segment in the wrong place.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reordered_frame_stops_the_receive() {
    let dir = tempfile::tempdir().unwrap();
    let home = Home::open(&dir.path().join("home")).unwrap();
    let mut fixture = fixture(&dir, &home).await;
    let first = seal(&mut fixture, 1).await;
    let second = seal(&mut fixture, 2).await;
    fixture.log.push(first);
    fixture.log.push(second);
    fixture.log.swap(0, 1);
    let err = receive(&fixture, &dir).await.unwrap_err();
    assert!(matches!(err, Error::Verify(_)), "{err}");
    assert!(!dir.path().join("out.bin").exists());
    fixture.provider.shutdown().await.unwrap();
}

/// A live receive stops at `--max-size` with a `Storage` error, and the bytes that arrived stay as
/// the start of the file: a stream that never closes has no other end.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_size_bound_stops_the_stream_and_keeps_the_prefix() {
    let dir = tempfile::tempdir().unwrap();
    let home = Home::open(&dir.path().join("home")).unwrap();
    let mut fixture = fixture(&dir, &home).await;
    let first = seal(&mut fixture, 3).await;
    let second = seal(&mut fixture, 4).await;
    fixture.log.push(first);
    fixture.log.push(second);

    let out = dir.path().join("out.bin");
    let store_root = dir.path().join("live-store");
    fs::create_dir(&store_root).unwrap();
    let mut options = options(&out);
    // One segment fits and the second would pass it.
    options.max_size = Some((3 << 20) / 2);
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let err = receive_live(&fixture.ticket.encode(), &options, &store_root, &tx)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Storage(_)), "{err}");
    assert!(err.to_string().contains("--max-size"), "{err}");
    assert_eq!(
        fs::read(&out).unwrap().len(),
        1 << 20,
        "the prefix that arrived was not kept"
    );
    assert!(
        fs::read_dir(&store_root).unwrap().next().is_none(),
        "a temporary store was kept"
    );
    fixture.provider.shutdown().await.unwrap();
}

/// The free-space floor stops a live receive through the same hook a fixed transfer uses, so a
/// stream cannot fill the volume the file is written to.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_free_space_floor_stops_the_stream() {
    let dir = tempfile::tempdir().unwrap();
    let home = Home::open(&dir.path().join("home")).unwrap();
    let mut fixture = fixture(&dir, &home).await;
    let frame = seal(&mut fixture, 5).await;
    fixture.log.push(frame);

    let out = dir.path().join("out.bin");
    let store_root = dir.path().join("live-store");
    fs::create_dir(&store_root).unwrap();
    let mut options = options(&out);
    // Exactly the floor, so the next segment would leave less than it.
    options.free_bytes = Some(1 << 30);
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let err = receive_live(&fixture.ticket.encode(), &options, &store_root, &tx)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Storage(_)), "{err}");
    assert!(err.to_string().contains("free"), "{err}");
    assert_eq!(fs::read(&out).unwrap().len(), 0);
    fixture.provider.shutdown().await.unwrap();
}

/// A file that shrinks while it is followed ends the stream with a storage error instead of closing
/// over bytes that are no longer the source, and the sender reports it rather than a finished send.
/// A follower that failed otherwise leaves every reader waiting for a close that never comes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_file_that_shrank_while_followed_ends_the_stream_with_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let home = Home::open(&dir.path().join("home")).unwrap();
    let source = dir.path().join("shrink.bin");
    fs::write(&source, vec![1u8; 2 << 20]).unwrap();
    let outgoing = prepare_live(
        &home,
        &source,
        RelayModeChoice::Disabled,
        Congestion::Cubic,
        None,
        None,
        Duration::from_secs(30),
    )
    .await
    .unwrap();
    let log = outgoing.live.as_ref().unwrap().log.clone();
    // Let the follower read the file, then take bytes away under it.
    tokio::time::sleep(Duration::from_millis(300)).await;
    fs::write(&source, b"short").unwrap();

    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let served = tokio::time::timeout(Duration::from_secs(10), outgoing.serve(Keep::One, &tx))
        .await
        .expect("the stream did not end");
    assert!(matches!(served, Err(Error::Storage(_))));
    let message = log.error().unwrap_or_default();
    assert!(message.contains("shrank"), "{message}");
}

#[test]
fn one_segment_is_the_size_the_spec_names() {
    assert_eq!(SEGMENT_BYTES, 8 << 20);
}

/// Every byte the store folder holds, except its database: the blobs themselves.
fn store_bytes(dir: &Path) -> u64 {
    let mut total = 0;
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            total += store_bytes(&path);
        } else if path.file_name().is_none_or(|name| name != "blobs.db") {
            total += entry.metadata().map(|meta| meta.len()).unwrap_or(0);
        }
    }
    total
}

/// Waits until the store folder holds bytes matching `done`, and returns what it held then. The
/// deadline is generous because a machine running the whole suite at once schedules the store's
/// collector late; what this waits for is the bound, not a speed.
async fn wait_for_store(dir: &Path, done: impl Fn(u64) -> bool) -> u64 {
    let deadline = std::time::Instant::now() + Duration::from_secs(90);
    loop {
        let bytes = store_bytes(dir);
        if done(bytes) {
            return bytes;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the store held {bytes} bytes and did not change"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// A live send keeps the segments it seals in a store of its own inside the home's temporary area,
/// one copy of each and no more, and when it has served its close the whole folder goes: the home
/// holds none of the send's bytes afterwards, and the `blobs` folder an older home kept them in is
/// not there at all.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_live_send_keeps_its_segments_in_its_own_folder_and_leaves_none_behind() {
    // Five full segments; the last, short one is the tail and is sealed only at the close.
    const SEALED: u64 = 5 * (8 << 20);
    const CONTENT: u64 = SEALED + (1 << 20);
    let dir = tempfile::tempdir().unwrap();
    let home = Home::open(&dir.path().join("home")).unwrap();
    let source = dir.path().join("long.bin");
    // Segments that differ from each other: five copies of one segment would be one blob.
    let mut body = vec![0u8; CONTENT as usize];
    for (at, byte) in body.iter_mut().enumerate() {
        *byte = (at % 251) as u8;
    }
    fs::write(&source, body.clone()).unwrap();
    let outgoing = prepare_live(
        &home,
        &source,
        RelayModeChoice::Disabled,
        Congestion::Cubic,
        None,
        None,
        // Long enough that the follower seals every full segment before it closes the stream.
        Duration::from_millis(300),
    )
    .await
    .unwrap();
    let scratch = outgoing
        .scratch()
        .expect("a live send has a store folder")
        .to_path_buf();
    assert_eq!(
        scratch.parent(),
        Some(home.temporary_folder().unwrap().as_path()),
        "the store is not in the home's temporary area"
    );
    // Five full segments, held for the readers that have not asked for them yet.
    let sealed = wait_for_store(&scratch, |bytes| bytes >= SEALED).await;
    assert!(
        sealed < SEALED + (4 << 20),
        "the store held {sealed} bytes for {SEALED} bytes of sealed segments"
    );

    // The send serves the stream to a reader and stops at its close, which is the path that closes
    // the store and removes the folder.
    let text = outgoing.ticket.encode();
    let (serve_tx, _serve_rx) = tokio::sync::mpsc::unbounded_channel();
    let serving = tokio::spawn(async move { outgoing.serve(Keep::One, &serve_tx).await });
    let out = dir.path().join("out.bin");
    let store_root = dir.path().join("receive-store");
    fs::create_dir(&store_root).unwrap();
    let (recv_tx, _recv_rx) = tokio::sync::mpsc::unbounded_channel();
    let received = receive_live(&text, &options(&out), &store_root, &recv_tx)
        .await
        .unwrap();
    assert_eq!(received.bytes, CONTENT);
    assert_eq!(fs::read(&out).unwrap(), body);
    assert_eq!(serving.await.unwrap().unwrap(), StopEnd::Downloaded);
    assert!(!scratch.exists(), "the send kept its store folder");
    assert_eq!(send_bytes(&home), 0, "the send left bytes in the home");
    assert!(
        !home.path().join("blobs").exists(),
        "the home still has a blobs folder"
    );
}
