//! Rebuilding one file from the candidate and the ranges fetched for it, and putting the result in
//! the store only if it is the file the manifest names.
//!
//! The steps for one file: prove the size and ask for the root (the plain protocol's own first
//! request), read the signatures, cut the candidate with the sender's average, plan, send the
//! ranges that are still needed, fetch them into the store, and assemble the file in the store's
//! temporary area from the candidate's bytes and the fetched ones. The assembly is hashed and
//! compared with the root before it is imported. Every step that does not work ends the attempt
//! and leaves the plain fetch to do the file, with nothing of the attempt left behind but the
//! ranges it fetched, which the plain fetch reuses.

use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use iroh_blobs::{
    Hash,
    api::blobs::{AddPathOptions, ImportMode},
    protocol::{ChunkRanges, ChunkRangesExt},
    store::fs::FsStore,
};
use sluice_delta::{Plan, Span};
use tokio::sync::mpsc::UnboundedSender;

use super::{BLOCK, Candidate, MAX_NEED_RANGES, WORTH_EIGHTHS, ask};
use crate::Error;
use crate::engine::{Event, Rate, Receiver, fetch};
use crate::ticket::Ticket;

/// What the files of one receive took from the candidate and from the network, summed.
#[derive(Debug, Default)]
pub(in crate::engine) struct Totals {
    local: AtomicU64,
    network: AtomicU64,
    signatures: AtomicU64,
}

impl Totals {
    /// Notes `bytes` of a file that were fetched whole, because no delta was used for it.
    pub(in crate::engine) fn fetched_whole(&self, bytes: u64) {
        self.network.fetch_add(bytes, Ordering::SeqCst);
    }

    /// The sums as the report a receive hands back.
    pub(in crate::engine) fn report(&self) -> crate::engine::DeltaReport {
        crate::engine::DeltaReport {
            local_bytes: self.local.load(Ordering::SeqCst),
            network_bytes: self.network.load(Ordering::SeqCst),
            signature_bytes: self.signatures.load(Ordering::SeqCst),
        }
    }
}

/// Everything one file's attempt needs.
pub(in crate::engine) struct Job<'a> {
    pub(in crate::engine) receiver: &'a Receiver,
    pub(in crate::engine) ticket: &'a Ticket,
    pub(in crate::engine) store: &'a FsStore,
    pub(in crate::engine) conn: &'a iroh::endpoint::Connection,
    pub(in crate::engine) rate: Option<&'a Rate>,
    pub(in crate::engine) events: &'a UnboundedSender<Event>,
    pub(in crate::engine) totals: &'a Totals,
    /// The candidate file for this entry.
    pub(in crate::engine) candidate: &'a Path,
    /// The store's temporary area, where the assembly is written.
    pub(in crate::engine) work: &'a Path,
}

/// Tries to put the blob `root`, `size` bytes, in the store by a delta. `true` is a complete blob in
/// the store, verified against `root`. `false` is every other outcome, and the caller fetches it.
pub(in crate::engine) async fn fetch_file(job: &Job<'_>, root: [u8; 32], size: u64) -> bool {
    matches!(attempt(job, root, size).await, Ok(true))
}

async fn attempt(job: &Job<'_>, root: [u8; 32], size: u64) -> Result<bool, Error> {
    let hash = Hash::from(root);
    let Some(candidate) = Candidate::open(job.candidate) else {
        return Ok(false);
    };
    // The request that proves the size also tells the sender's policy this endpoint asked for the
    // root, which is what lets it be told about the root's chunks.
    if fetch::verified_size(job.store, job.conn, hash).await? != size {
        return Ok(false);
    }
    let Some((control, list, signature_bytes)) =
        ask::signatures(job.receiver, job.ticket, root, size, job.events).await
    else {
        return Ok(false);
    };
    let planned = plan_for(&candidate, &list, size).await;
    let Some(plan) = planned.filter(|plan| worth_it(plan, size)) else {
        control.close();
        return Ok(false);
    };
    let Ok(need) = plan.need_ranges(root) else {
        control.close();
        return Ok(false);
    };
    control.need(&need).await;
    let fetched = fetch_needed(job, hash, &need).await;
    control.close();
    fetched?;
    let rebuilt = assemble(job, &candidate, &plan, hash, size).await;
    if !matches!(rebuilt, Ok(true)) {
        return Ok(false);
    }
    job.totals
        .local
        .fetch_add(plan.local_bytes(), Ordering::SeqCst);
    job.totals
        .network
        .fetch_add(plan.fetch_bytes(), Ordering::SeqCst);
    job.totals
        .signatures
        .fetch_add(signature_bytes, Ordering::SeqCst);
    Ok(true)
}

/// The plan for `list` against `candidate`, cut with the list's average.
async fn plan_for(candidate: &Candidate, list: &sluice_delta::SigList, size: u64) -> Option<Plan> {
    let params = sluice_delta::Params::new(list.avg_bytes()).ok()?;
    let chunks = candidate.chunks(params).await?;
    sluice_delta::plan(list, &chunks, size).ok()
}

/// Whether a plan saves enough to be worth assembling: at least an eighth of the file comes from the
/// candidate, and the request is of a size one frame carries.
fn worth_it(plan: &Plan, size: u64) -> bool {
    let ranges = plan
        .spans()
        .iter()
        .filter(|span| matches!(span, Span::Fetch(_)))
        .count();
    plan.local_bytes().saturating_mul(8) >= size.saturating_mul(WORTH_EIGHTHS)
        && ranges <= MAX_NEED_RANGES
}

/// Fetches the ranges a plan needs into the store, verified block by block against the root.
async fn fetch_needed(
    job: &Job<'_>,
    hash: Hash,
    need: &sluice_delta::NeedRanges,
) -> Result<(), Error> {
    let mut ranges = ChunkRanges::empty();
    for range in need.ranges() {
        ranges |= ChunkRanges::bytes(range.start()..range.end());
    }
    if ranges.is_empty() {
        return Ok(());
    }
    fetch::fetch_range(job.store, job.conn, hash, ranges, job.rate).await
}

/// A file in the temporary area that is removed when it is dropped, whatever became of it.
struct Assembly(PathBuf);

impl Drop for Assembly {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// Writes the file from the candidate's bytes and the fetched ones, checks its hash against the
/// root, and imports it. `true` is a complete blob that has the root's hash.
async fn assemble(
    job: &Job<'_>,
    candidate: &Candidate,
    plan: &Plan,
    hash: Hash,
    size: u64,
) -> Result<bool, Error> {
    let io = |e: io::Error| Error::Storage(format!("cannot assemble a delta: {e}"));
    fs::create_dir_all(job.work).map_err(io)?;
    let path = job.work.join(format!("{hash}.part"));
    let assembly = Assembly(path.clone());
    let file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&path)
        .map_err(io)?;
    file.set_len(size).map_err(io)?;
    let file = std::sync::Arc::new(file);
    copy_local(candidate, plan, &file).await.map_err(io)?;
    copy_fetched(job.store, hash, plan, &file)
        .await
        .map_err(io)?;
    // A candidate that changed while it was read gives an assembly that fails the hash below; the
    // look here only saves the hash.
    let same = candidate.unchanged() && hashes_to(&path, hash).await.map_err(io)?;
    if !same {
        return Ok(false);
    }
    let tag = job
        .store
        .blobs()
        .add_path_with_opts(AddPathOptions {
            path: path.clone(),
            format: iroh_blobs::BlobFormat::Raw,
            mode: ImportMode::Copy,
        })
        .temp_tag()
        .await
        .map_err(|e| Error::Storage(format!("cannot import a delta: {}", crate::plain(&e))))?;
    // The store has its own copy now: a reference would keep the holes of the partial entry that
    // the fetched ranges made, and the file in the work area is removed when `assembly` drops.
    drop(assembly);
    let imported = tag.hash() == hash;
    Ok(imported)
}

/// Copies every local span from the candidate into the assembly, a block at a time.
async fn copy_local(
    candidate: &Candidate,
    plan: &Plan,
    file: &std::sync::Arc<fs::File>,
) -> io::Result<()> {
    let spans: Vec<(u64, u64, u64)> = plan
        .spans()
        .iter()
        .filter_map(|span| match span {
            Span::Local { target, candidate } => Some((target.start(), target.len(), *candidate)),
            Span::Fetch(_) => None,
        })
        .collect();
    let candidate = candidate.clone();
    let file = std::sync::Arc::clone(file);
    tokio::task::spawn_blocking(move || {
        let mut buffer = vec![0u8; BLOCK];
        for (start, len, from) in spans {
            let mut done = 0u64;
            while done < len {
                let n = usize::try_from((len - done).min(BLOCK as u64)).unwrap_or(BLOCK);
                candidate.read_exact_at(from + done, &mut buffer[..n])?;
                write_all_at(&file, &buffer[..n], start + done)?;
                done += n as u64;
            }
        }
        Ok(())
    })
    .await
    .map_err(io::Error::other)?
}

/// Copies every fetched span from the store into the assembly. The store holds them verified.
async fn copy_fetched(
    store: &FsStore,
    hash: Hash,
    plan: &Plan,
    file: &std::sync::Arc<fs::File>,
) -> io::Result<()> {
    for span in plan.spans() {
        let Span::Fetch(range) = span else { continue };
        super::export(store, hash, range.start()..range.end(), |offset, piece| {
            write_all_at(file, piece, offset)
        })
        .await?;
    }
    Ok(())
}

/// Whether the file at `path` hashes to `hash`.
async fn hashes_to(path: &Path, hash: Hash) -> io::Result<bool> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        use std::io::Read;
        let mut file = fs::File::open(path)?;
        let mut hasher = blake3::Hasher::new();
        let mut buffer = vec![0u8; BLOCK];
        loop {
            let n = file.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            hasher.update(&buffer[..n]);
        }
        Ok(hasher.finalize().as_bytes() == hash.as_bytes())
    })
    .await
    .map_err(io::Error::other)?
}

#[cfg(unix)]
fn write_all_at(file: &fs::File, buf: &[u8], offset: u64) -> io::Result<()> {
    std::os::unix::fs::FileExt::write_all_at(file, buf, offset)
}

#[cfg(windows)]
fn write_all_at(file: &fs::File, mut buf: &[u8], mut offset: u64) -> io::Result<()> {
    while !buf.is_empty() {
        let n = std::os::windows::fs::FileExt::seek_write(file, buf, offset)?;
        if n == 0 {
            return Err(io::ErrorKind::WriteZero.into());
        }
        buf = &buf[n..];
        offset += n as u64;
    }
    Ok(())
}
