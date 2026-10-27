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

mod checks;
mod rules;
mod server;
mod transfer;

use super::{
    ALPN, Congestion, Event, Hello, ReceiveOptions, ServeEnd, Stop, StopEnd, greet, hello, now_ms,
    prepare_send, receive,
};

fn options(out: &Path) -> super::ReceiveOptions {
    super::ReceiveOptions {
        out: out.to_path_buf(),
        force: false,
        no_times: false,
        links: false,
        allow_hidden: false,
        limits: Limits::default(),
        free_bytes: None,
        congestion: super::Congestion::Cubic,
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
    let home = Home::open(&dir.path().join("home")).unwrap();
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
