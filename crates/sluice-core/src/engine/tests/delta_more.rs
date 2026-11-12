//! The edges of a delta: identical files, tiny candidates and targets, a hash mismatch that leaves
//! nothing behind, an update in place, a list that lies about the length, and links on the way to
//! the candidate.

use std::time::Duration;

use sluice_delta::{Entry, Params, SigList};

use super::delta::{SIZE, bytes, delta_options, sender, stopped};
use super::*;
use crate::engine::hello::varint_bytes;

/// A folder or file link: a symbolic link on Unix and a junction on Windows, which needs no
/// privilege.
fn link_dir(target: &Path, link: &Path) {
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, link).unwrap();
    #[cfg(windows)]
    {
        let made = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .output()
            .unwrap();
        assert!(made.status.success(), "mklink /J failed");
    }
}

/// A candidate that is the file itself: the plan needs no range and every byte comes from it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_identical_file_is_rebuilt_without_a_range() {
    let dir = tempfile::tempdir().unwrap();
    let new = bytes(SIZE, 41);
    let from = dir.path().join("same.bin");
    fs::write(&from, &new).unwrap();
    let sender = sender(&dir, &new).await;
    let out = dir.path().join("received");
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let received = super::receive(&sender.text, &delta_options(&out, &from), &tx)
        .await
        .unwrap();
    assert_eq!(fs::read(out.join("source.bin")).unwrap(), new);
    let report = received.delta.unwrap();
    assert_eq!((report.local_bytes, report.network_bytes), (SIZE as u64, 0));
    let needs = sender.delta.needs();
    assert_eq!(needs.len(), 1);
    assert!(needs[0].ranges().is_empty(), "a range was asked for");
    stopped(sender).await;
}

/// A candidate smaller than one chunk of the shortest length has nothing to match, and the file
/// arrives plainly.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_candidate_smaller_than_a_chunk_is_no_help() {
    let dir = tempfile::tempdir().unwrap();
    let new = bytes(SIZE, 42);
    let from = dir.path().join("tiny.bin");
    fs::write(&from, &new[..100]).unwrap();
    let sender = sender(&dir, &new).await;
    let out = dir.path().join("received");
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let received = super::receive(&sender.text, &delta_options(&out, &from), &tx)
        .await
        .unwrap();
    assert_eq!(fs::read(out.join("source.bin")).unwrap(), new);
    let report = received.delta.unwrap();
    assert_eq!((report.local_bytes, report.network_bytes), (0, SIZE as u64));
    stopped(sender).await;
}

/// The list of a blob of exactly 65,536 bytes is written, and one of 65,535 is not: a list is for a
/// blob of at least that many.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_smallest_blob_with_a_list_is_65536_bytes() {
    for (len, listed) in [(65_536usize, true), (65_535, false)] {
        let dir = tempfile::tempdir().unwrap();
        let body = bytes(len, 43);
        let root = *blake3::hash(&body).as_bytes();
        let (outgoing, text) = prepared(&dir, &body).await;
        let serving = tokio::spawn(async move { outgoing.serve(Keep::One, &no_events()).await });
        let ticket = Ticket::decode(&text).unwrap();
        let asker = Receiver::open(RelayModeChoice::Disabled, super::Congestion::Cubic)
            .await
            .unwrap();
        let _store = super::receipts::admit(&dir, &asker, &ticket, root).await;
        let told = crate::engine::delta::ask::signatures(
            &asker,
            &ticket,
            root,
            len as u64,
            Duration::from_secs(5),
            &no_events(),
        )
        .await;
        assert_eq!(told.is_some(), listed, "{len} bytes");
        asker.shutdown().await.unwrap();
        serving.abort();
    }
}

/// A list whose lengths are not the length of the content is refused by the requester that asked.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_list_of_another_length_is_refused_by_the_ask() {
    let dir = tempfile::tempdir().unwrap();
    let new = bytes(SIZE, 44);
    let root = *blake3::hash(&new).as_bytes();
    let other = bytes(SIZE + 1000, 45);
    let params = Params::default();
    let entries = params
        .chunks(&other)
        .map(|chunk| Entry::of_bytes(chunk).unwrap())
        .collect();
    let list = SigList::new(params.avg(), entries).unwrap();
    let mut frame = varint_bytes(list.encode().len() as u64);
    frame.extend_from_slice(&list.encode());
    let sender = sender(&dir, &new).await;
    sender.delta.force_frame(frame);
    let ticket = Ticket::decode(&sender.text).unwrap();
    let asker = Receiver::open(RelayModeChoice::Disabled, super::Congestion::Cubic)
        .await
        .unwrap();
    let _store = super::receipts::admit(&dir, &asker, &ticket, root).await;
    let told = crate::engine::delta::ask::signatures(
        &asker,
        &ticket,
        root,
        SIZE as u64,
        Duration::from_secs(5),
        &no_events(),
    )
    .await;
    assert!(told.is_none(), "a list for other content was taken");
    asker.shutdown().await.unwrap();
    sender.serve.abort();
}

/// A hash mismatch leaves no assembly in the work folder: the kept store of a cut-off receive holds
/// no `.part` file.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_mismatch_leaves_no_assembly_behind() {
    let dir = tempfile::tempdir().unwrap();
    let new = bytes(SIZE, 46);
    let mut old = new.clone();
    old[1 << 20] ^= 0xff;
    let params = Params::default();
    let entries = params
        .chunks(&old)
        .map(|chunk| Entry::of_bytes(chunk).unwrap())
        .collect();
    let list = SigList::new(params.avg(), entries).unwrap();
    let mut frame = varint_bytes(list.encode().len() as u64);
    frame.extend_from_slice(&list.encode());
    let from = dir.path().join("old.bin");
    fs::write(&from, &old).unwrap();
    let sender = sender(&dir, &new).await;
    sender.delta.force_frame(frame);
    let out = dir.path().join("received");
    let mut options = delta_options(&out, &from);
    // A full disk after the content is in place keeps the store, and with it whatever the delta
    // left in its work folder.
    options.write_limit = Some(256 * 1024);
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    super::receive(&sender.text, &options, &tx)
        .await
        .unwrap_err();
    let partial = fs::read_dir(&out)
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .find(|path| {
            path.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with(".sluice-partial-"))
        })
        .expect("the store was kept");
    let work = partial.join("delta");
    if let Ok(entries) = fs::read_dir(&work) {
        let left: Vec<_> = entries.flatten().map(|e| e.file_name()).collect();
        assert!(left.is_empty(), "{left:?} was left in the work folder");
    }
    sender.serve.abort();
}

/// A plan with more ranges than a request may carry is not used: the file is fetched plainly.
#[test]
fn a_plan_with_too_many_ranges_is_not_worth_it() {
    let params = Params::new(256).unwrap();
    let count = crate::engine::delta::MAX_NEED_RANGES + 100;
    let strong = |n: u64| -> [u8; 16] {
        let mut out = [0u8; 16];
        out[..8].copy_from_slice(&n.to_le_bytes());
        out
    };
    // Every other chunk of the target is one the candidate holds, so each unmatched chunk between
    // them is a range of its own.
    let mut list = Vec::new();
    let mut held = Vec::new();
    for n in 0..count as u64 {
        held.push(Entry::new(100, strong(n)).unwrap());
        list.push(Entry::new(100, strong(n)).unwrap());
        list.push(Entry::new(100, strong(1 << 40 | n)).unwrap());
    }
    let size = 200 * count as u64;
    let list = SigList::new(params.avg(), list).unwrap();
    let candidate = sluice_delta::Candidate::from_entries(&params, held);
    let plan = sluice_delta::plan(&list, &candidate, size).unwrap();
    assert!(
        plan.local_bytes() * 2 >= size - 200,
        "half the file is local"
    );
    assert!(!crate::engine::delta::worth_it(&plan, size));
}

/// An update in place: the old file is both the candidate and the output, with `--force`. The
/// candidate is read before the output is replaced, and the new file is what is left.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_update_in_place_replaces_the_candidate_it_read() {
    let dir = tempfile::tempdir().unwrap();
    let new = bytes(SIZE, 47);
    let mut old = new.clone();
    for byte in &mut old[1 << 20..(1 << 20) + 40_000] {
        *byte ^= 0x11;
    }
    let out = dir.path().join("received");
    fs::create_dir_all(&out).unwrap();
    let target = out.join("source.bin");
    fs::write(&target, &old).unwrap();
    let sender = sender(&dir, &new).await;
    let mut options = delta_options(&out, &target);
    options.force = true;
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let received = super::receive(&sender.text, &options, &tx).await.unwrap();
    assert_eq!(fs::read(&target).unwrap(), new);
    let report = received.delta.unwrap();
    assert!(report.local_bytes > SIZE as u64 / 2, "{report:?}");
    stopped(sender).await;
}

/// A link on the way to the candidate, or at its name, is not used: the file arrives plainly and
/// the sender was asked for no list.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_link_on_the_way_or_at_the_name_is_not_a_candidate() {
    let dir = tempfile::tempdir().unwrap();
    let new = bytes(SIZE, 48);
    let tree = dir.path().join("tree");
    fs::create_dir_all(&tree).unwrap();
    fs::write(tree.join("big.bin"), &new).unwrap();
    let real = dir.path().join("real");
    fs::create_dir_all(&real).unwrap();
    fs::write(real.join("big.bin"), &new).unwrap();

    // `have/tree` is a link to a folder that holds the right file.
    let have = dir.path().join("have");
    fs::create_dir_all(&have).unwrap();
    link_dir(&real, &have.join("tree"));
    // `lone/source.bin` is a link where a file belongs, for a transfer of one file.
    let lone = dir.path().join("lone");
    fs::create_dir_all(&lone).unwrap();
    link_dir(&real, &lone.join("source.bin"));

    let (outgoing, text) = prepared_paths(&dir, std::slice::from_ref(&tree)).await;
    let delta = std::sync::Arc::clone(outgoing.provider.delta().unwrap());
    let serving = tokio::spawn(async move { outgoing.serve(Keep::One, &no_events()).await });
    let out = dir.path().join("received");
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let received = super::receive(&text, &delta_options(&out, &have), &tx)
        .await
        .unwrap();
    assert_eq!(fs::read(out.join("tree").join("big.bin")).unwrap(), new);
    let report = received.delta.unwrap();
    assert_eq!((report.local_bytes, report.network_bytes), (0, SIZE as u64));
    assert_eq!(delta.served(), 0, "a link on the way was used");
    serving.abort();

    let dir2 = tempfile::tempdir().unwrap();
    let sender = sender(&dir2, &new).await;
    let out = dir2.path().join("received");
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let received = super::receive(&sender.text, &delta_options(&out, &lone), &tx)
        .await
        .unwrap();
    assert_eq!(fs::read(out.join("source.bin")).unwrap(), new);
    assert_eq!(received.delta.unwrap().local_bytes, 0);
    assert_eq!(sender.delta.served(), 0, "a link at the name was used");
    stopped(sender).await;
}
