//! End-to-end checks against a real endpoint on loopback. They use a temporary home and store and
//! no relay, so they run on a machine with no internet.

use std::{
    collections::BTreeSet,
    fs,
    path::Path,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use iroh::Endpoint;
use iroh_blobs::{
    Hash,
    api::proto::BlobStatus,
    protocol::{
        ChunkRanges, ChunkRangesSeq, GetManyRequest, GetRequest, ObserveRequest, PushRequest,
        RequestType,
    },
    store::{fs::FsStore, mem::MemStore},
};
use tempfile::TempDir;

use super::{Provider, Receiver, RelayModeChoice, endpoint};
use crate::{
    Error, Home,
    manifest::Limits,
    ticket::{Body, Format, Ticket},
};

struct Fixture {
    _dir: TempDir,
    _home: Home,
    provider: Provider,
    hash: Hash,
    endpoint: Endpoint,
}

async fn fixture(expiry: Option<u64>) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let home = Home::open(&dir.path().join("home")).unwrap();
    let store = FsStore::load(dir.path().join("store")).await.unwrap();
    let hash = store.add_slice(b"a small file").await.unwrap().hash;
    let provider = Provider::start(
        &home,
        store,
        BTreeSet::from([*hash.as_bytes()]),
        RelayModeChoice::Disabled,
        super::Congestion::Cubic,
        expiry,
    )
    .await
    .unwrap();
    let endpoint = endpoint::bind(None, &RelayModeChoice::Disabled, super::Congestion::Cubic)
        .await
        .unwrap();
    Fixture {
        _dir: dir,
        _home: home,
        provider,
        hash,
        endpoint,
    }
}

impl Fixture {
    async fn connect(&self) -> iroh::endpoint::Connection {
        self.endpoint
            .connect(self.provider.addr(), iroh_blobs::ALPN)
            .await
            .unwrap()
    }

    async fn close(self) {
        self.endpoint.close().await;
        self.provider.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn an_allowed_blob_is_served_and_verifies() {
    let f = fixture(None).await;
    let client = MemStore::new();
    let stats = client
        .remote()
        .execute_get(f.connect().await, GetRequest::blob(f.hash))
        .complete()
        .await
        .unwrap();
    assert!(stats.payload_bytes_read > 0);
    assert_eq!(
        client.get_bytes(f.hash).await.unwrap().as_ref(),
        b"a small file"
    );
    f.close().await;
}

#[tokio::test]
async fn a_hash_the_send_does_not_offer_is_refused() {
    let f = fixture(None).await;
    let client = MemStore::new();
    let err = client
        .remote()
        .execute_get(f.connect().await, GetRequest::blob(Hash::from([7; 32])))
        .complete()
        .await
        .unwrap_err();
    assert!(!err.to_string().is_empty());
    f.close().await;
}

#[tokio::test]
async fn get_many_cannot_bypass_the_policy() {
    let f = fixture(None).await;
    let client = MemStore::new();
    let request = GetManyRequest::builder()
        .hash(Hash::from([8; 32]), ChunkRanges::all())
        .build();
    let err = client
        .remote()
        .execute_get_many(f.connect().await, request)
        .complete()
        .await
        .unwrap_err();
    assert!(!err.to_string().is_empty());
    f.close().await;
}

#[tokio::test]
async fn observe_cannot_bypass_the_policy() {
    let f = fixture(None).await;
    let conn = f.connect().await;
    let (mut send, mut recv) = conn.open_bi().await.unwrap();
    let request = ObserveRequest::new(Hash::from([9; 32]));
    send.write_all(&[RequestType::Observe as u8]).await.unwrap();
    send.write_all(&postcard::to_allocvec(&request).unwrap())
        .await
        .unwrap();
    send.finish().unwrap();
    let err = recv.read_to_end(1).await.unwrap_err();
    assert_ne!(err.to_string(), "");
    f.close().await;
}

#[tokio::test]
async fn a_push_never_reaches_the_provider_store() {
    let f = fixture(None).await;
    let source = MemStore::new();
    let pushed = source.add_slice(b"do not take this").await.unwrap().hash;
    let _ = source
        .remote()
        .execute_push(
            f.connect().await,
            PushRequest::new(pushed, ChunkRangesSeq::root()),
        )
        .complete()
        .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(matches!(
        f.provider.store().blobs().status(pushed).await.unwrap(),
        BlobStatus::NotFound
    ));
    f.close().await;
}

#[tokio::test]
async fn an_expired_ticket_is_refused_before_the_dial() {
    let f = fixture(Some(0)).await;
    let client = MemStore::new();
    let err = client
        .remote()
        .execute_get(f.connect().await, GetRequest::blob(f.hash))
        .complete()
        .await
        .unwrap_err();
    assert!(!err.to_string().is_empty());
    f.close().await;
}

#[tokio::test]
async fn a_receiver_refuses_an_expired_ticket() {
    let receiver = Receiver::open(RelayModeChoice::Disabled, super::Congestion::Cubic)
        .await
        .unwrap();
    let ticket = Ticket::new(
        [1; 32],
        vec![],
        None,
        Some(0),
        crate::ticket::Body::Static {
            root: [2; 32],
            size: 1,
            format: crate::ticket::Format::Raw,
        },
    )
    .unwrap();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let err = receiver.connect(&ticket, tx).await.unwrap_err();
    assert!(matches!(err, crate::Error::Policy(_)));
    receiver.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn one_file_goes_out_and_arrives_with_its_name() {
    let dir = tempfile::tempdir().unwrap();
    let home = Home::open(&dir.path().join("home")).unwrap();
    let source = dir.path().join("holiday photo.jpg");
    let body = b"a tiny file that fits in one pack";
    std::fs::write(&source, body).unwrap();

    let outgoing = super::prepare_send(
        &home,
        &source,
        RelayModeChoice::Disabled,
        super::Congestion::Cubic,
        None,
    )
    .await
    .unwrap();
    let text = outgoing.ticket.encode();
    let (serve_tx, _serve_rx) = tokio::sync::mpsc::unbounded_channel();
    let serve = tokio::spawn(async move { outgoing.serve(false, &serve_tx).await });

    let out = dir.path().join("received");
    let (recv_tx, _recv_rx) = tokio::sync::mpsc::unbounded_channel();
    let received = super::receive(
        &text,
        &super::ReceiveOptions {
            out: out.clone(),
            force: false,
            no_times: false,
            links: false,
            limits: crate::manifest::Limits::default(),
            free_bytes: None,
            congestion: super::Congestion::Cubic,
        },
        &recv_tx,
    )
    .await
    .unwrap();
    assert_eq!(received.bytes, body.len() as u64);
    assert_eq!(received.path, out.join("holiday photo.jpg"));
    assert_eq!(std::fs::read(&received.path).unwrap(), body);
    let end = serve.await.unwrap().unwrap();
    assert_eq!(end, super::StopEnd::Downloaded);
}

#[tokio::test]
async fn the_greeting_is_exchanged_both_ways() {
    let f = fixture(None).await;
    let conn = f
        .endpoint
        .connect(f.provider.addr(), super::ALPN)
        .await
        .unwrap();
    let theirs = super::greet(&conn).await.unwrap();
    assert_eq!(theirs, super::Hello::ours());
    conn.close(0u32.into(), b"done");
    f.close().await;
}

#[tokio::test]
async fn another_version_is_refused() {
    let f = fixture(None).await;
    let conn = f
        .endpoint
        .connect(f.provider.addr(), super::ALPN)
        .await
        .unwrap();
    let (mut send, mut recv) = conn.open_bi().await.unwrap();
    let hello = super::Hello {
        proto_version: 2,
        ..super::Hello::ours()
    };
    send.write_all(&super::hello::encode(hello)).await.unwrap();
    send.finish().unwrap();
    assert!(super::hello::read_hello(&mut recv).await.is_err());
    conn.close(0u32.into(), b"done");
    f.close().await;
}

fn options(out: &Path) -> super::ReceiveOptions {
    super::ReceiveOptions {
        out: out.to_path_buf(),
        force: false,
        no_times: false,
        links: false,
        limits: Limits::default(),
        free_bytes: None,
        congestion: super::Congestion::Cubic,
    }
}

async fn prepared(dir: &TempDir, body: &[u8]) -> (super::Outgoing, String) {
    let home = Home::open(&dir.path().join("home")).unwrap();
    let source = dir.path().join("source.bin");
    fs::write(&source, body).unwrap();
    let outgoing = super::prepare_send(
        &home,
        &source,
        RelayModeChoice::Disabled,
        super::Congestion::Cubic,
        None,
    )
    .await
    .unwrap();
    let text = outgoing.ticket.encode();
    (outgoing, text)
}

async fn partial_store(out: &Path) -> FsStore {
    let entry = fs::read_dir(out)
        .unwrap()
        .flatten()
        .find(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(".sluice-partial-")
        })
        .expect("a partial store was kept");
    FsStore::load(entry.path()).await.unwrap()
}

#[tokio::test]
async fn a_full_disk_is_refused_before_the_content_is_asked_for() {
    let dir = tempfile::tempdir().unwrap();
    let body = vec![7u8; 2 << 20];
    let (outgoing, text) = prepared(&dir, &body).await;
    let out = dir.path().join("out");
    let mut options = options(&out);
    options.free_bytes = Some(0);
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let err = super::receive(&text, &options, &tx).await.unwrap_err();
    assert!(matches!(err, Error::Storage(_)), "{err}");
    let store = partial_store(&out).await;
    let content = Hash::from(*blake3::hash(&body).as_bytes());
    assert_eq!(
        store.blobs().status(content).await.unwrap(),
        BlobStatus::NotFound
    );
    store.shutdown().await.unwrap();
    outgoing.provider.shutdown().await.unwrap();
}

#[tokio::test]
async fn an_oversized_manifest_is_refused_after_its_size_is_proved() {
    let dir = tempfile::tempdir().unwrap();
    let home = Home::open(&dir.path().join("home")).unwrap();
    let store = FsStore::load(dir.path().join("sender-store"))
        .await
        .unwrap();
    let body = vec![9u8; 1 << 20];
    let hash = store.add_slice(&body).await.unwrap().hash;
    let provider = Provider::start(
        &home,
        store,
        BTreeSet::from([*hash.as_bytes()]),
        RelayModeChoice::Disabled,
        super::Congestion::Cubic,
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
            root: *hash.as_bytes(),
            size: body.len() as u64,
            format: Format::HashSequence,
        },
    )
    .unwrap();
    let out = dir.path().join("out");
    let mut options = options(&out);
    options.limits = Limits {
        max_bytes: 64 << 10,
        ..Limits::default()
    };
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let err = super::receive(&ticket.encode(), &options, &tx)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Policy(_)), "{err}");
    let store = partial_store(&out).await;
    assert!(matches!(
        store.blobs().status(hash).await.unwrap(),
        BlobStatus::Partial { .. }
    ));
    store.shutdown().await.unwrap();
    provider.shutdown().await.unwrap();
}

fn set_mtime(path: &Path, at: SystemTime) {
    let file = fs::File::options().write(true).open(path).unwrap();
    file.set_times(fs::FileTimes::new().set_modified(at))
        .unwrap();
}

fn mtime_ms(path: &Path) -> u64 {
    fs::metadata(path)
        .unwrap()
        .modified()
        .unwrap()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

#[tokio::test]
async fn the_time_is_restored_unless_told_not_to() {
    let dir = tempfile::tempdir().unwrap();
    let home = Home::open(&dir.path().join("home")).unwrap();
    let source = dir.path().join("source.bin");
    let body = b"a file with a known time";
    fs::write(&source, body).unwrap();
    let at = UNIX_EPOCH + Duration::from_secs(1_600_000_000);
    set_mtime(&source, at);
    let wanted = mtime_ms(&source);
    let outgoing = super::prepare_send(
        &home,
        &source,
        RelayModeChoice::Disabled,
        super::Congestion::Cubic,
        None,
    )
    .await
    .unwrap();
    let text = outgoing.ticket.encode();
    let out = dir.path().join("out");
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let received = super::receive(&text, &options(&out), &tx).await.unwrap();
    assert_eq!(mtime_ms(&received.path), wanted);
    outgoing.provider.shutdown().await.unwrap();

    let outgoing = super::prepare_send(
        &home,
        &source,
        RelayModeChoice::Disabled,
        super::Congestion::Cubic,
        None,
    )
    .await
    .unwrap();
    let text = outgoing.ticket.encode();
    let out = dir.path().join("untimed");
    let mut untimed = options(&out);
    untimed.no_times = true;
    let received = super::receive(&text, &untimed, &tx).await.unwrap();
    assert_ne!(mtime_ms(&received.path), wanted);
    outgoing.provider.shutdown().await.unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn mode_bits_are_applied_without_setuid_or_setgid() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let home = Home::open(&dir.path().join("home")).unwrap();
    let source = dir.path().join("source.bin");
    let body = b"an executable";
    fs::write(&source, body).unwrap();
    fs::set_permissions(&source, fs::Permissions::from_mode(0o6755)).unwrap();
    assert_eq!(
        fs::metadata(&source).unwrap().permissions().mode() & 0o6000,
        0o6000
    );
    let outgoing = super::prepare_send(
        &home,
        &source,
        RelayModeChoice::Disabled,
        super::Congestion::Cubic,
        None,
    )
    .await
    .unwrap();
    let text = outgoing.ticket.encode();
    let (serve_tx, _serve_rx) = tokio::sync::mpsc::unbounded_channel();
    let serve = tokio::spawn(async move { outgoing.serve(false, &serve_tx).await });
    let out = dir.path().join("out");
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let received = super::receive(&text, &options(&out), &tx).await.unwrap();
    let mode = fs::metadata(&received.path).unwrap().permissions().mode() & 0o7777;
    assert_eq!(mode, 0o755);
    serve.await.unwrap().unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn force_replaces_a_symlink_and_leaves_its_target() {
    use std::os::unix::fs::symlink;

    let dir = tempfile::tempdir().unwrap();
    let (outgoing, text) = prepared(&dir, b"the new body").await;
    let out = dir.path().join("out");
    fs::create_dir(&out).unwrap();
    let target = out.join("target");
    fs::write(&target, b"keep me").unwrap();
    symlink("target", out.join("source.bin")).unwrap();
    let mut options = options(&out);
    options.force = true;
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let received = super::receive(&text, &options, &tx).await.unwrap();
    assert!(fs::symlink_metadata(&received.path).unwrap().is_file());
    assert_eq!(fs::read(&target).unwrap(), b"keep me");
    assert_eq!(fs::read(&received.path).unwrap(), b"the new body");
    outgoing.provider.shutdown().await.unwrap();
}
