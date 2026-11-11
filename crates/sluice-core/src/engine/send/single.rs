//! One regular file, or standard input, from the sender's side: import it, build the one-entry
//! manifest (or none, for a raw pipe), and prepare the ticket.

mod entry;

use std::{fs, path::Path};

use iroh_blobs::{
    BlobFormat,
    api::{
        TempTag,
        blobs::{AddPathOptions, ImportMode},
    },
    store::fs::FsStore,
};

use super::{Congestion, Outgoing, Provider, RelayModeChoice, Scratch, serving, storage};
use crate::{
    Error, Home,
    manifest::Manifest,
    ticket::{Body, Format, Ticket},
};
use entry::{allowed_roots, import, keep_metadata};

pub(in crate::engine) use entry::{checked_file, content_roots, one_entry_manifest};

/// Imports `path` in place and starts a provider for it. `expiry` is UTC milliseconds or none.
///
/// Fails with [`Error::Policy`] for a folder, a symlink or a name a receiver would reject, and
/// with [`Error::Verify`] if the file changes between the import and the start of serving.
pub async fn prepare(
    home: &Home,
    path: &Path,
    relay: RelayModeChoice,
    congestion: Congestion,
    expiry_ms: Option<u64>,
    port: Option<u16>,
) -> Result<Outgoing, Error> {
    prepare_with_rate(home, path, relay, congestion, expiry_ms, port, None).await
}

/// [`prepare`] with a byte rate for serving the file.
pub async fn prepare_with_rate(
    home: &Home,
    path: &Path,
    relay: RelayModeChoice,
    congestion: Congestion,
    expiry_ms: Option<u64>,
    port: Option<u16>,
    rate: Option<std::sync::Arc<crate::engine::Rate>>,
) -> Result<Outgoing, Error> {
    let (shown, name, entry_path, before) = checked_file(path)?;
    let scratch = Scratch::open(home, storage::Collector::Off).await?;
    let tag = import(path, &shown, scratch.store()).await?;
    let hash = tag.hash();
    if !before.same(path)? {
        return Err(Error::Verify(format!(
            "{shown} changed while it was read, so its root names no version of it"
        )));
    }
    let manifest = one_entry_manifest(entry_path, &before, hash)?;
    let metadata = keep_metadata(scratch.store(), &manifest).await?;
    let relay_text = relay_text(&relay);
    let allowed = allowed_roots(&manifest);
    let provider = serving(
        home, scratch, allowed, relay, congestion, expiry_ms, port, rate,
    )
    .await?;
    let ticket = match ticket_of(&provider, &manifest, before.size, expiry_ms, relay_text) {
        Ok(ticket) => ticket,
        Err(e) => {
            // Stopping the provider closes the store and removes the folder with it.
            let _ = provider.shutdown().await;
            return Err(e);
        }
    };
    // The ticket is readable by anyone who has it, so it is printed and never traced.
    Ok(Outgoing {
        provider,
        ticket,
        roots: content_roots(&manifest),
        name: Some(name),
        files: 1,
        bytes: before.size,
        congestion,
        live: None,
        _tags: {
            let mut tags = vec![tag];
            tags.extend(metadata);
            tags
        },
    })
}

/// Sends standard input as a raw blob, ticket format 0. The bytes are read to the end before the
/// root is known, so they are copied into the store rather than referenced, and the pipe closes
/// before the ticket is printed.
pub async fn prepare_stdin(
    home: &Home,
    relay: RelayModeChoice,
    congestion: Congestion,
    expiry_ms: Option<u64>,
    port: Option<u16>,
) -> Result<Outgoing, Error> {
    prepare_stdin_with_rate(home, relay, congestion, expiry_ms, port, None).await
}

/// [`prepare_stdin`] with a byte rate for serving the blob.
pub async fn prepare_stdin_with_rate(
    home: &Home,
    relay: RelayModeChoice,
    congestion: Congestion,
    expiry_ms: Option<u64>,
    port: Option<u16>,
    rate: Option<std::sync::Arc<crate::engine::Rate>>,
) -> Result<Outgoing, Error> {
    prepare_stdin_body(
        home,
        std::io::stdin(),
        relay,
        congestion,
        expiry_ms,
        port,
        rate,
    )
    .await
}

/// [`prepare_stdin_with_rate`] with the stream to read as an argument, so a test sends bytes without
/// a pipe. The bytes are read to the end before the root is known, so they are copied into the
/// send's own store, and the stream is closed before the ticket exists.
async fn prepare_stdin_body(
    home: &Home,
    from: impl std::io::Read + Send + 'static,
    relay: RelayModeChoice,
    congestion: Congestion,
    expiry_ms: Option<u64>,
    port: Option<u16>,
    rate: Option<std::sync::Arc<crate::engine::Rate>>,
) -> Result<Outgoing, Error> {
    let scratch = Scratch::open(home, storage::Collector::Off).await?;
    let (tag, size) = import_stdin(scratch.path(), scratch.store(), from).await?;
    let hash = tag.hash();
    let allowed = std::collections::BTreeSet::from([*hash.as_bytes()]);
    let relay_text = relay_text(&relay);
    let provider = serving(
        home, scratch, allowed, relay, congestion, expiry_ms, port, rate,
    )
    .await?;
    let ticket = match ticket_with_body(
        &provider,
        expiry_ms,
        relay_text,
        Body::Static {
            root: *hash.as_bytes(),
            size,
            format: Format::Raw,
        },
    ) {
        Ok(ticket) => ticket,
        Err(e) => {
            // Stopping the provider closes the store and removes the folder with it.
            let _ = provider.shutdown().await;
            return Err(e);
        }
    };
    Ok(Outgoing {
        provider,
        ticket,
        roots: vec![*hash.as_bytes()],
        name: None,
        files: 1,
        bytes: size,
        congestion,
        live: None,
        _tags: vec![tag],
    })
}

/// Reads standard input into a file of the send's own folder and copies it into the store, so the
/// pipe closes before the ticket exists and the bytes survive the reader going away. The file is
/// removed once the store holds the bytes, so the folder holds one copy of them.
async fn import_stdin(
    dir: &Path,
    store: &FsStore,
    from: impl std::io::Read + Send + 'static,
) -> Result<(TempTag, u64), Error> {
    let path = dir.join("input");
    let size = read_input(&path, from).await?;
    let copied = store
        .blobs()
        .add_path_with_opts(AddPathOptions {
            path: path.clone(),
            format: BlobFormat::Raw,
            mode: ImportMode::Copy,
        })
        .temp_tag()
        .await;
    let tag = copied
        .map_err(|e| Error::Storage(format!("cannot keep standard input: {}", crate::plain(&e))))?;
    let _ = fs::remove_file(&path);
    Ok((tag, size))
}

/// Reads a stream to a file on disk, so a large pipe never sits in memory.
async fn read_input(
    path: &Path,
    mut from: impl std::io::Read + Send + 'static,
) -> Result<u64, Error> {
    let target = path.to_path_buf();
    let read = tokio::task::spawn_blocking(move || {
        let mut file = fs::File::create(&target)?;
        let size = std::io::copy(&mut from, &mut file)?;
        file.sync_all()?;
        Ok::<u64, std::io::Error>(size)
    })
    .await
    .map_err(|e| Error::Storage(format!("reading standard input stopped: {e}")))?;
    read.map_err(|e| storage::storage("read standard input into", path, e))
}

/// A ticket for a body other than a manifest sequence, with the provider's addresses and relay.
fn ticket_with_body(
    provider: &Provider,
    expiry_ms: Option<u64>,
    relay_text: Option<String>,
    body: Body,
) -> Result<Ticket, Error> {
    let addr = provider.addr();
    let relay_url = relay_text.or_else(|| {
        addr.relay_urls()
            .next()
            .map(std::string::ToString::to_string)
    });
    Ticket::new(
        *addr.id.as_bytes(),
        addr.ip_addrs().copied().collect(),
        relay_url,
        expiry_ms,
        body,
    )
}

fn relay_text(relay: &RelayModeChoice) -> Option<String> {
    match relay {
        RelayModeChoice::Disabled | RelayModeChoice::N0 => None,
        RelayModeChoice::Url(url) => Some(url.clone()),
    }
}

fn ticket_of(
    provider: &Provider,
    manifest: &Manifest,
    size: u64,
    expiry_ms: Option<u64>,
    relay_text: Option<String>,
) -> Result<Ticket, Error> {
    let addr = provider.addr();
    let relay_url = relay_text.or_else(|| {
        addr.relay_urls()
            .next()
            .map(std::string::ToString::to_string)
    });
    Ticket::new(
        *addr.id.as_bytes(),
        addr.ip_addrs().copied().collect(),
        relay_url,
        expiry_ms,
        Body::Static {
            root: manifest.sequence_root(),
            size,
            format: Format::HashSequence,
        },
    )
}

/// A send of standard input whose bytes come from memory rather than a pipe, for a test that has no
/// pipe to write to.
#[cfg(test)]
pub(crate) async fn prepare_stdin_from(home: &Home, bytes: Vec<u8>) -> Result<Outgoing, Error> {
    prepare_stdin_body(
        home,
        std::io::Cursor::new(bytes),
        RelayModeChoice::Disabled,
        Congestion::Cubic,
        None,
        None,
        None,
    )
    .await
}
