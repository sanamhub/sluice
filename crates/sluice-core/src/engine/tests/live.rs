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
    let serving = tokio::spawn(async move { outgoing.serve(false, &tx).await });

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
    let provider = Provider::start_live(
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

#[test]
fn one_segment_is_the_size_the_spec_names() {
    assert_eq!(SEGMENT_BYTES, 8 << 20);
}
