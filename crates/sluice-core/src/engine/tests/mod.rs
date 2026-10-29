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
        ChunkRanges, ChunkRangesExt, ChunkRangesSeq, GetManyRequest, GetRequest, ObserveRequest,
        PushRequest, RequestType,
    },
    store::{fs::FsStore, mem::MemStore},
};
use tempfile::TempDir;

use super::{Provider, Receiver, RelayModeChoice, endpoint};
use crate::{
    Error, Home,
    manifest::{Entry, Kind, Limits, Loc, Manifest, path::ManifestPath},
    ticket::{Body, Format, Ticket},
};

mod cancel;
mod checks;
mod done;
mod inspect;
mod lifecycle;
mod live;
mod resume;
mod rules;
mod server;
mod transfer;

use super::{
    ALPN, Congestion, Event, Hello, ReceiveOptions, ServeEnd, Stop, StopEnd, greet, hello, now_ms,
    prepare_send, receive,
};

/// Every byte a send left in the home: what is under its temporary area and in the `blobs` folder an
/// older home kept its imports in. The home's own files are not a send's.
fn send_bytes(home: &Home) -> u64 {
    fn walk(dir: &Path) -> u64 {
        let Ok(entries) = fs::read_dir(dir) else {
            return 0;
        };
        let mut total = 0;
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = fs::symlink_metadata(&path) else {
                continue;
            };
            if meta.is_dir() {
                total += walk(&path);
            } else {
                total += meta.len();
            }
        }
        total
    }
    walk(&home.path().join("tmp")) + walk(&home.path().join("blobs"))
}
fn options(out: &Path) -> super::ReceiveOptions {
    super::ReceiveOptions {
        out: out.to_path_buf(),
        force: false,
        no_times: false,
        links: false,
        allow_hidden: false,
        limits: Limits::default(),
        free_bytes: None,
        max_size: None,
        congestion: super::Congestion::Cubic,
        rate: None,
        write_limit: None,
        max_network_failures: crate::queue::MAX_NETWORK_FAILURES,
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
        None,
    )
    .await
    .unwrap();
    let text = outgoing.ticket.encode();
    (outgoing, text)
}

async fn prepared_paths(dir: &TempDir, paths: &[std::path::PathBuf]) -> (super::Outgoing, String) {
    prepared_paths_in(dir, "home", paths).await
}

/// [`prepared_paths`] with the name of the home under `dir`, so a test can have two senders at once:
/// one store is open to one process at a time.
async fn prepared_paths_in(
    dir: &TempDir,
    home: &str,
    paths: &[std::path::PathBuf],
) -> (super::Outgoing, String) {
    let home = Home::open(&dir.path().join(home)).unwrap();
    let outgoing = super::prepare_send_paths(
        &home,
        paths,
        RelayModeChoice::Disabled,
        super::Congestion::Cubic,
        None,
        None,
    )
    .await
    .unwrap();
    let text = outgoing.ticket.encode();
    (outgoing, text)
}

/// Whether the output folder holds the partial store of a receive.
fn partial_exists(out: &Path) -> bool {
    fs::read_dir(out).unwrap().flatten().any(|entry| {
        entry
            .file_name()
            .to_string_lossy()
            .starts_with(".sluice-partial-")
    })
}

/// Every hidden name a receive made must be gone after a refusal, including its temporary folder.
fn assert_no_partial(out: &Path) {
    for entry in fs::read_dir(out).unwrap().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        assert!(
            !name.starts_with(".sluice-partial-") && !name.starts_with(".sluice-tmp-"),
            "{name} was kept"
        );
    }
}

/// A client endpoint and its own store, for a test that talks to a provider by hand.
async fn client_of(dir: &TempDir) -> (Endpoint, FsStore) {
    let client = endpoint::bind(
        None,
        &RelayModeChoice::Disabled,
        super::Congestion::Cubic,
        None,
    )
    .await
    .unwrap();
    let store = FsStore::load(dir.path().join("client-store"))
        .await
        .unwrap();
    (client, store)
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

/// A provider for a manifest that declares `declared` bytes for a blob that really has `body`.
/// Nothing checks those two agree on the sending side, because a hostile sender need not.
async fn provider_of_lying_manifest(
    dir: &TempDir,
    body: &[u8],
    declared: u64,
    mtime_ms: u64,
) -> (Provider, Hash, Ticket) {
    let home = Home::open(&dir.path().join("lying-home")).unwrap();
    let store = FsStore::load(dir.path().join("lying-store")).await.unwrap();
    let hash = store.add_slice(body).await.unwrap().hash;
    let manifest = Manifest::new(
        vec![Entry {
            path: ManifestPath::parse("big.bin").unwrap(),
            kind: Kind::File,
            size: declared,
            mtime_ms,
            mode: 0,
            loc: Loc::Blob {
                root: *hash.as_bytes(),
            },
            link_target: None,
        }],
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
            size: declared,
            format: Format::HashSequence,
        },
    )
    .unwrap();
    (provider, hash, ticket)
}
