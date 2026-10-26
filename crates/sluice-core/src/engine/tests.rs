//! End-to-end checks against a real endpoint on loopback. They use a temporary home and store and
//! no relay, so they run on a machine with no internet.

use std::{collections::BTreeSet, time::Duration};

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
use crate::{Home, ticket::Ticket};

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
        expiry,
    )
    .await
    .unwrap();
    let endpoint = endpoint::bind(None, &RelayModeChoice::Disabled)
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
    let receiver = Receiver::open(RelayModeChoice::Disabled).await.unwrap();
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

    let outgoing = super::prepare_send(&home, &source, RelayModeChoice::Disabled, None)
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
