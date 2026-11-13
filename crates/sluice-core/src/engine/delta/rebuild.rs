//! Rebuilding one file from the candidate and the ranges fetched for it, and putting the result in
//! the store only if it is the file the manifest names.
//!
//! The steps for one file: prove the size and ask for the root (the plain protocol's own first
//! request), read the signatures, cut the candidate with the sender's average, plan, send the
//! ranges that are still needed, fetch them into the store, and assemble the file in the store's
//! temporary area from the candidate's bytes and the fetched ones. The store's import hashes the
//! assembly once, and the blob is kept only if that hash is the root. Every step that does not work
//! ends the attempt and leaves the plain fetch to do the file, with nothing of the attempt left
//! behind but the ranges it fetched, which the plain fetch reuses.

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

use super::{BLOCK, Candidate, MAX_NEED_RANGES, WORTH_EIGHTHS, ask, params_for};
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
    /// How long to wait for the list, which a test shortens.
    pub(in crate::engine) wait: Option<std::time::Duration>,
    /// What the files before this one have moved, and what the whole receive moves, for progress.
    pub(in crate::engine) base: u64,
    pub(in crate::engine) total: u64,
}

/// Tries to put the blob `root`, `size` bytes, in the store by a delta. `true` is a complete blob in
/// the store, verified against `root`. `false` is every other outcome, and the caller fetches it.
pub(in crate::engine) async fn fetch_file(job: &Job<'_>, root: [u8; 32], size: u64) -> bool {
    matches!(attempt(job, root, size).await, Ok(true))
}

/// The cut of the candidate that is started with a guess of the sender's average while the first
/// requests are in flight. It stops when this is dropped, and [`Self::finish`] waits for it to have
/// stopped, so the file is not held open past the attempt.
struct EarlyCut {
    params: sluice_delta::Params,
    handle: Option<tokio::task::JoinHandle<Option<sluice_delta::Candidate>>>,
    stop: super::candidate::StopOnDrop,
}

impl EarlyCut {
    fn start(candidate: &Candidate, params: sluice_delta::Params) -> Self {
        let stop = super::candidate::StopOnDrop::default();
        let (candidate, flag) = (candidate.clone(), std::sync::Arc::clone(&stop.0));
        let handle = tokio::spawn(async move {
            candidate
                .chunks_until(params, &flag, std::time::Duration::ZERO)
                .await
        });
        Self {
            params,
            handle: Some(handle),
            stop,
        }
    }

    /// Stops the cut and waits until its thread has let go of the file.
    async fn finish(&mut self) {
        self.stop.0.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.await;
        }
    }
}

async fn attempt(job: &Job<'_>, root: [u8; 32], size: u64) -> Result<bool, Error> {
    let Some(candidate) = Candidate::open(job.candidate).await else {
        return Ok(false);
    };
    let mut early = params_for(size, crate::engine::hello::MAX_FRAME_BYTES)
        .map(|params| EarlyCut::start(&candidate, params));
    let outcome = run(job, root, size, &candidate, &mut early).await;
    // Whatever the outcome, the cut has stopped and let go of the file before the write phase.
    if let Some(early) = early.as_mut() {
        early.finish().await;
    }
    outcome
}

async fn run(
    job: &Job<'_>,
    root: [u8; 32],
    size: u64,
    candidate: &Candidate,
    early: &mut Option<EarlyCut>,
) -> Result<bool, Error> {
    let hash = Hash::from(root);
    // The request that proves the size also tells the sender's policy this endpoint asked for the
    // root, which is what lets it be told about the root's chunks.
    if fetch::verified_size(job.store, job.conn, hash).await? != size {
        return Ok(false);
    }
    let wait = job.wait.unwrap_or_else(|| super::control_wait(size));
    let Some((control, list, signature_bytes)) =
        ask::signatures(job.receiver, job.ticket, root, size, wait, job.events).await
    else {
        return Ok(false);
    };
    // A list that arrived cost its bytes, whatever becomes of the attempt.
    job.totals
        .signatures
        .fetch_add(signature_bytes, Ordering::SeqCst);
    let planned = plan_for(candidate, &list, size, early).await;
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
    // The ranges came over the network, and stay counted if the file then fails its hash.
    job.totals
        .network
        .fetch_add(plan.fetch_bytes(), Ordering::SeqCst);
    let rebuilt = assemble(job, candidate, &plan, hash, size).await;
    if !matches!(rebuilt, Ok(true)) {
        return Ok(false);
    }
    job.totals
        .local
        .fetch_add(plan.local_bytes(), Ordering::SeqCst);
    Ok(true)
}

/// The plan for `list` against `candidate`, cut with the list's average. The early cut is used when
/// its guess of the average was right, and is stopped and replaced by a cut with the list's when it
/// was not.
async fn plan_for(
    candidate: &Candidate,
    list: &sluice_delta::SigList,
    size: u64,
    early: &mut Option<EarlyCut>,
) -> Option<Plan> {
    let params = sluice_delta::Params::new(list.avg_bytes()).ok()?;
    let guessed = early.as_ref().is_some_and(|cut| cut.params == params);
    let chunks = match early.as_mut() {
        Some(cut) if guessed => cut.handle.take()?.await.ok().flatten()?,
        _ => {
            if let Some(cut) = early.as_mut() {
                cut.finish().await;
            }
            candidate.chunks(params).await?
        }
    };
    sluice_delta::plan(list, &chunks, size).ok()
}

/// Whether a plan saves enough to be worth assembling: at least an eighth of the file comes from the
/// candidate, and the request is of a size one frame carries.
pub(in crate::engine) fn worth_it(plan: &Plan, size: u64) -> bool {
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
    let (events, base, total) = (job.events, job.base, job.total);
    fetch::fetch_range_reporting(job.store, job.conn, hash, ranges, job.rate, |seen| {
        if total > 0 {
            crate::engine::emit(
                events,
                Event::Progress {
                    bytes: base.saturating_add(seen).min(total),
                    total,
                },
            );
        }
    })
    .await
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
    // A candidate that changed while it was read gives an assembly whose hash is not the root's;
    // the look here only saves the import.
    if !candidate.unchanged() {
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
    // The store hashed the file as it imported it, and that hash is the only check: the blob is the
    // file the manifest names when it has the root's hash. The store has its own copy now, and the
    // file in the work area goes whatever the hash was.
    drop(assembly);
    let imported = tag.hash() == hash;
    if imported && job.total > 0 {
        crate::engine::emit(
            job.events,
            Event::Progress {
                bytes: job.base.saturating_add(size).min(job.total),
                total: job.total,
            },
        );
    }
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
