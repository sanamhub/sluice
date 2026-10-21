//! A tree of 100,000 files, built and timed. It is slow to set up and says nothing a normal run
//! needs, so it is ignored. Run it by hand in release mode:
//!
//! `cargo test -p sluice-core --release -- --ignored --nocapture a_tree_of_100000_files`

use std::fs;
use std::time::Instant;

use crate::manifest::{Build, Limits, Manifest};

const FOLDERS: usize = 1000;
const FILES_PER_FOLDER: usize = 100;

#[test]
#[ignore = "builds 100,000 files; run by hand with --ignored --nocapture"]
fn a_tree_of_100000_files_is_built_and_timed() {
    let dir = tempfile::tempdir().expect("a temporary folder");
    let started = Instant::now();
    for folder in 0..FOLDERS {
        let path = dir.path().join(format!("dir{folder:04}"));
        fs::create_dir(&path).expect("create a folder");
        for file in 0..FILES_PER_FOLDER {
            // A few hundred bytes each, different in each, so no two files share a root.
            let content = format!("file {folder} {file} {}", "x".repeat(200));
            fs::write(path.join(format!("file{file:03}.txt")), content).expect("write a file");
        }
    }
    eprintln!("created the tree in {:?}", started.elapsed());

    let started = Instant::now();
    let Build::Ready(built) = Manifest::build(dir.path()).expect("the build runs") else {
        panic!("nothing in the tree is refused");
    };
    let built_in = started.elapsed();
    let entries = built.manifest.entries().len();
    assert_eq!(entries, FOLDERS * (FILES_PER_FOLDER + 1));

    let started = Instant::now();
    let bytes = built.manifest.encode();
    let encoded_in = started.elapsed();
    let started = Instant::now();
    let decoded = Manifest::decode(&bytes, Limits::default()).expect("a receiver accepts it");
    let decoded_in = started.elapsed();
    assert_eq!(decoded, built.manifest);

    let packs = built.manifest.packs();
    eprintln!(
        "BUILD {entries} entries ({FOLDERS} folders, {} files): {built_in:?}; {} packs, {} B; \
         manifest {} bytes; encode {encoded_in:?}; decode with the collision check {decoded_in:?}",
        FOLDERS * FILES_PER_FOLDER,
        packs.len(),
        packs.iter().map(|p| p.size).sum::<u64>(),
        bytes.len(),
    );
}
