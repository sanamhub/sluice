//! The choices that do not need a connection: which average a size is cut with, and what a receive
//! holds for a target of any size.

use sluice_delta::{AVG_BYTES, MAX_ENTRIES};

use super::{CANDIDATE_ENTRIES, MAX_NEED_RANGES, MAX_SIG_FRAME, params_for};
use crate::engine::hello::MAX_FRAME_BYTES;

const GIB: u64 = 1 << 30;

/// A file that fits the default average is cut with it, and a larger one with the smallest average
/// whose worst case still fits: the sender never writes a list a receiver must refuse.
#[test]
fn a_size_is_cut_with_the_smallest_average_that_fits_a_frame() {
    assert_eq!(params_for(GIB, MAX_FRAME_BYTES).unwrap().avg(), AVG_BYTES);
    let big = params_for(100 * GIB, MAX_FRAME_BYTES).unwrap();
    assert_eq!(big.avg(), 512 << 10);
    let smaller = params_for(100 * GIB, MAX_FRAME_BYTES).unwrap().avg() / 2;
    let worst = (100 * GIB).div_ceil(u64::from(smaller / 4));
    assert!(worst > 838_860, "a smaller average would not have fit");
}

/// For every size the list the sender writes fits the receiver's frame and the entry limit, even
/// when the content cuts into the shortest chunks the average allows.
#[test]
fn the_worst_case_list_always_fits_the_frame_and_the_limit() {
    for size in [1u64 << 16, 1 << 20, GIB, 10 * GIB, 100 * GIB, 600 * GIB] {
        let params = params_for(size, MAX_FRAME_BYTES).expect("a size within the limit");
        let worst = size.div_ceil(u64::from(params.min()));
        assert!(worst <= u64::from(MAX_ENTRIES), "{size}");
        assert!(15 + 20 * worst <= u64::from(MAX_FRAME_BYTES), "{size}");
    }
}

/// A target too large for the biggest average is not signed, and a receiver with a small frame limit
/// gets a coarser list or none.
#[test]
fn a_size_no_average_fits_is_not_signed() {
    assert!(params_for(4 * 1024 * GIB, MAX_FRAME_BYTES).is_none());
    let small = params_for(GIB, 1 << 20).unwrap();
    assert!(small.avg() > AVG_BYTES);
    assert!(params_for(GIB, 100).is_none());
}

/// What a receive holds does not depend on the target: these are the numbers the module says, and a
/// 100 GiB target is inside every one of them.
#[test]
fn what_a_receive_holds_is_bounded_whatever_the_target() {
    // The decoded list: 24 bytes an entry, at most the limit.
    const { assert!(24 * MAX_ENTRIES as u64 <= 25 << 20) };
    const { assert!(MAX_SIG_FRAME <= 21 << 20) };
    // The candidate table and its index: 32 bytes and about 48 bytes an entry.
    const { assert!((32 + 48) * CANDIDATE_ENTRIES as u64 <= 170 << 20) };
    // The largest chunk any average cuts, which is the most the cutter buffers beyond a block.
    assert_eq!(
        u64::from(sluice_delta::Params::new(4 << 20).unwrap().max()),
        32 << 20
    );
    // The need-ranges one request holds.
    const { assert!(16 * MAX_NEED_RANGES as u64 <= 1 << 20) };
    // A 100 GiB target needs a list no bigger than any other target's.
    let params = params_for(100 * GIB, MAX_FRAME_BYTES).unwrap();
    assert!((100 * GIB).div_ceil(u64::from(params.min())) <= u64::from(MAX_ENTRIES));
}

/// A candidate is read only inside the length it had at the open, whatever the file does after: a
/// file that grows is cut at the old length, a read past it is refused, and a file that shrinks is
/// cut where it ended without a panic and without a read of what is no longer there.
#[tokio::test]
async fn a_candidate_is_read_only_inside_the_length_it_had() {
    use std::io::Write;

    use super::Candidate;

    let params = sluice_delta::Params::new(1024).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("candidate.bin");
    let body: Vec<u8> = (0..300_000u32).map(|n| (n * 7 % 251) as u8).collect();
    std::fs::write(&path, &body).unwrap();
    let candidate = Candidate::open(&path).expect("a regular file opens");

    // Grown after the open: the extra bytes are not read.
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    file.write_all(&[9; 100_000]).unwrap();
    drop(file);
    let chunks = candidate.chunks(params).await.unwrap();
    assert_eq!(chunks.len(), 300_000);
    let mut buffer = [0u8; 20];
    assert!(candidate.read_exact_at(299_990, &mut buffer).is_err());
    candidate.read_exact_at(299_980, &mut buffer).unwrap();
    assert_eq!(buffer, body[299_980..300_000]);
    assert!(
        !candidate.unchanged(),
        "a grown file is not the file that was opened"
    );

    // Shrunk after the open: cut where it ends, and a read of the missing bytes fails.
    std::fs::write(&path, &body[..100_000]).unwrap();
    let chunks = candidate.chunks(params).await.unwrap();
    assert_eq!(chunks.len(), 100_000);
    assert!(candidate.read_exact_at(200_000, &mut buffer).is_err());
}

/// What is not a regular file is not opened: a missing path, a folder, and (where a link can be made)
/// a link at the name or on the way to the file.
#[test]
fn a_path_that_is_not_a_regular_file_is_not_a_candidate() {
    use super::Candidate;

    let dir = tempfile::tempdir().unwrap();
    assert!(Candidate::open(&dir.path().join("absent")).is_none());
    assert!(Candidate::open(dir.path()).is_none());
    let file = dir.path().join("real.bin");
    std::fs::write(&file, [1u8; 100]).unwrap();
    assert!(Candidate::open(&file).is_some());
    #[cfg(unix)]
    {
        let link = dir.path().join("link.bin");
        std::os::unix::fs::symlink(&file, &link).unwrap();
        assert!(
            Candidate::open(&link).is_none(),
            "a link at the name was followed"
        );
    }
}

/// A candidate is opened read-only: nothing written through the handle it holds can reach the file,
/// and opening and reading leaves the bytes and the modification time as they were.
#[tokio::test]
async fn a_candidate_is_never_modified() {
    use super::Candidate;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("candidate.bin");
    let body = vec![5u8; 200_000];
    std::fs::write(&path, &body).unwrap();
    let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
    let candidate = Candidate::open(&path).unwrap();
    candidate
        .chunks(sluice_delta::Params::new(1024).unwrap())
        .await
        .unwrap();
    let mut buffer = [0u8; 64];
    candidate.read_exact_at(0, &mut buffer).unwrap();
    drop(candidate);
    assert_eq!(std::fs::read(&path).unwrap(), body);
    assert_eq!(
        std::fs::metadata(&path).unwrap().modified().unwrap(),
        modified
    );
}
