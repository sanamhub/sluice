//! One byte range of a one-file transfer, through two real processes.

mod common;

use common::{data, recv_range, start, start_in};

/// The folders under the home's `tmp` now. A send keeps its own store there while it serves, so a
/// test of what a receive leaves counts what was there before the receive started.
fn temporary_stores(home: &std::path::Path) -> usize {
    std::fs::read_dir(home.join("tmp"))
        .map(|entries| entries.count())
        .unwrap_or(0)
}

#[test]
fn a_byte_range_writes_only_those_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let out = dir.path().join("out");
    let file = dir.path().join("range.bin");
    let body = data(300 * 1024, 12);
    std::fs::write(&file, &body).unwrap();

    let (sender, ticket) = start(&home, &file, "1h");
    let before = temporary_stores(&home);
    let result = recv_range(&home, &ticket, &out, "1000:2000");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        std::fs::read(out.join("range.bin")).unwrap(),
        &body[1000..2000]
    );
    // The sender's own store may already be gone by now; what this checks is that the receive added
    // a folder of its own and did not remove it.
    assert!(
        temporary_stores(&home) <= before,
        "the receive left a temporary store under the home"
    );
    // A range is not a whole download, so the sender is still serving here.
    drop(sender);
}

#[test]
fn a_range_across_a_block_boundary_keeps_the_block_seam() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let out = dir.path().join("out");
    let file = dir.path().join("seam.bin");
    let body = data(64 * 1024, 4);
    std::fs::write(&file, &body).unwrap();

    let (sender, ticket) = start(&home, &file, "1h");
    let before = temporary_stores(&home);
    let result = recv_range(&home, &ticket, &out, "16000:17000");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        std::fs::read(out.join("seam.bin")).unwrap(),
        &body[16_000..17_000]
    );
    // The sender's own store may already be gone by now; what this checks is that the receive added
    // a folder of its own and did not remove it.
    assert!(
        temporary_stores(&home) <= before,
        "the receive left a temporary store under the home"
    );
    drop(sender);
}

#[test]
fn a_range_of_a_packed_file_reads_out_of_its_pack() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let out = dir.path().join("out");
    std::fs::create_dir_all(dir.path().join("small")).unwrap();
    let file = dir.path().join("small").join("packed.txt");
    let body = data(5000, 6);
    std::fs::write(&file, &body).unwrap();

    let (sender, ticket) = start_in(dir.path(), &home, &["small/packed.txt", "--expires", "1h"]);
    let before = temporary_stores(&home);
    let result = recv_range(&home, &ticket, &out, "100:200");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        std::fs::read(out.join("packed.txt")).unwrap(),
        &body[100..200]
    );
    // The sender's own store may already be gone by now; what this checks is that the receive added
    // a folder of its own and did not remove it.
    assert!(
        temporary_stores(&home) <= before,
        "the receive left a temporary store under the home"
    );
    drop(sender);
}

#[test]
fn a_range_is_refused_on_a_folder() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let out = dir.path().join("out");
    let tree = dir.path().join("tree");
    std::fs::create_dir(&tree).unwrap();
    std::fs::write(tree.join("a.txt"), b"one").unwrap();

    let (sender, ticket) = start(&home, &tree, "1h");
    let result = recv_range(&home, &ticket, &out, "0:1");
    assert_eq!(result.status.code(), Some(6));
    assert!(String::from_utf8_lossy(&result.stderr).contains("folder"));
    drop(sender);
}
