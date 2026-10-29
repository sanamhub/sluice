//! The provider's side: what it serves, what it refuses, and how the greeting is checked.

use super::*;

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
    let mut provider = Provider::start(
        &home,
        store,
        BTreeSet::from([*hash.as_bytes()]),
        RelayModeChoice::Disabled,
        super::Congestion::Cubic,
        expiry,
        None,
    )
    .await
    .unwrap();
    // The fixture stands for a send that has printed its ticket, so its mode is set.
    provider.set_limit(None);
    let endpoint = endpoint::bind(
        None,
        &RelayModeChoice::Disabled,
        super::Congestion::Cubic,
        None,
    )
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
    assert_eq!(
        err.iroh_error_code(),
        Some(iroh_blobs::protocol::ERR_PERMISSION)
    );
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
    assert_eq!(
        err.iroh_error_code(),
        Some(iroh_blobs::protocol::ERR_PERMISSION)
    );
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
    assert!(
        matches!(
            err,
            noq::ReadToEndError::Read(noq::ReadError::Reset(code))
                if code == iroh_blobs::protocol::ERR_PERMISSION
        ),
        "{err:?}"
    );
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
    assert!(matches!(
        f.provider.store().blobs().status(pushed).await.unwrap(),
        BlobStatus::NotFound
    ));
    f.close().await;
}

#[tokio::test]
async fn an_expired_ticket_is_refused_by_the_provider() {
    let f = fixture(Some(0)).await;
    let client = MemStore::new();
    let err = client
        .remote()
        .execute_get(f.connect().await, GetRequest::blob(f.hash))
        .complete()
        .await
        .unwrap_err();
    assert_eq!(
        err.iroh_error_code(),
        Some(iroh_blobs::protocol::ERR_PERMISSION)
    );
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
#[tokio::test]
async fn get_many_serves_an_allowed_hash() {
    let f = fixture(None).await;
    let client = MemStore::new();
    let request = GetManyRequest::builder()
        .hash(f.hash, ChunkRanges::all())
        .build();
    client
        .remote()
        .execute_get_many(f.connect().await, request)
        .complete()
        .await
        .unwrap();
    assert_eq!(
        client.get_bytes(f.hash).await.unwrap().as_ref(),
        b"a small file"
    );
    f.close().await;
}

#[tokio::test]
async fn observe_reports_an_allowed_hash() {
    use n0_future::StreamExt;

    let f = fixture(None).await;
    let client = MemStore::new();
    let mut stream = client
        .remote()
        .observe(f.connect().await, ObserveRequest::new(f.hash));
    let first = stream.next().await.expect("an observe answer");
    assert!(first.is_ok(), "{:?}", first.err());
    f.close().await;
}
