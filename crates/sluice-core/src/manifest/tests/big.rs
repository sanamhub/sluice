//! A manifest of 1,000,000 entries, decoded and timed, to measure what the decoder holds on top
//! of the entries. It is slow and says nothing a normal run needs, so it is ignored. Run it by
//! hand in release mode and read the peak memory of the process from outside:
//!
//! `cargo test -p sluice-core --release --lib -- --ignored --nocapture a_manifest_of_1000000`
//!
//! The bytes are written here by hand, without building the entries first, so the only large
//! thing the process holds before the decode is the input, and its size is printed.

use std::time::Instant;

use crate::manifest::{Limits, Manifest};

const ENTRIES: usize = 1_000_000;
const FILES_PER_FOLDER: usize = 100;

/// A small xorshift, so the names are the same on every run and the test needs no dependency.
struct Names(u64);

impl Names {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    /// A name of 10 to 40 characters: letters, then the index, so no two in a folder are equal.
    fn name(&mut self, index: usize) -> String {
        let digits = format!("{index}");
        let len = 10 + (self.next() % 31) as usize;
        let mut name: String = (0..len - digits.len())
            .map(|_| char::from(b'a' + (self.next() % 26) as u8))
            .collect();
        name.push_str(&digits);
        name
    }
}

fn varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push((value as u8) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn entry(out: &mut Vec<u8>, path: &str, file: Option<(u64, u64)>) {
    varint(out, path.len() as u64);
    out.extend_from_slice(path.as_bytes());
    match file {
        // A file: size, time, mode 0o644, a Blob loc and its root.
        Some((size, root)) => {
            out.push(0);
            varint(out, size);
            varint(out, 1_800_000_000_000 + size);
            varint(out, 0o644);
            out.push(0);
            out.extend_from_slice(&[0; 24]);
            out.extend_from_slice(&root.to_le_bytes());
        }
        // A folder: size 0, a time, mode 0o755, a None loc.
        None => {
            out.push(1);
            varint(out, 0);
            varint(out, 1_800_000_000_000);
            varint(out, 0o755);
            out.push(2);
        }
    }
    out.push(0);
}

/// Folders of 100 files each, in manifest order. Every name is `[a-z0-9]`, which sorts after
/// `/`, so a folder's files come right after it once the folder names are sorted.
fn manifest_bytes() -> Vec<u8> {
    let mut names = Names(0x9e37_79b9_7f4a_7c15);
    let folders = ENTRIES.div_ceil(FILES_PER_FOLDER + 1);
    let mut folder_names: Vec<String> = (0..folders).map(|i| names.name(i)).collect();
    folder_names.sort();
    let mut out = Vec::new();
    out.push(1);
    varint(&mut out, ENTRIES as u64);
    let mut written = 0;
    for folder in &folder_names {
        entry(&mut out, folder, None);
        written += 1;
        let files = FILES_PER_FOLDER.min(ENTRIES - written);
        let mut file_names: Vec<String> = (0..files).map(|i| names.name(i)).collect();
        file_names.sort();
        for file in &file_names {
            let size = names.next() % (1 << 20);
            entry(
                &mut out,
                &format!("{folder}/{file}"),
                Some((size, names.next())),
            );
        }
        written += files;
    }
    assert_eq!(written, ENTRIES);
    varint(&mut out, 0);
    out.shrink_to_fit();
    out
}

#[test]
#[ignore = "decodes 1,000,000 entries; run by hand with --ignored --nocapture"]
fn a_manifest_of_1000000_entries_is_decoded_and_timed() {
    let bytes = manifest_bytes();
    let started = Instant::now();
    let manifest = Manifest::decode(&bytes, Limits::default()).expect("a receiver accepts it");
    let decoded_in = started.elapsed();
    assert_eq!(manifest.entries().len(), ENTRIES);
    eprintln!(
        "DECODE {ENTRIES} entries from {} bytes (capacity {}): {decoded_in:?}",
        bytes.len(),
        bytes.capacity()
    );
}
