use std::collections::BTreeSet;
use std::fs;

use super::{Target, run, seeds, vector_files};
use crate::conformance::vectors_dir;

const WITH_VECTORS: [(Target, &str); 4] = [
    (Target::Ticket, "tickets"),
    (Target::Manifest, "manifests"),
    (Target::ManifestPath, "paths"),
    (Target::Log, "live"),
];

#[test]
fn every_target_that_reads_vectors_gets_a_seed_from_each_file_it_reads() {
    for (target, folder) in WITH_VECTORS {
        let files = vector_files(&vectors_dir().join(folder)).unwrap();
        assert!(!files.is_empty(), "{folder} has no vector files");
        let seeded: BTreeSet<String> = seeds(target, &vectors_dir())
            .unwrap()
            .into_iter()
            .filter_map(|seed| seed.file)
            .collect();
        for (name, _) in files {
            assert!(
                seeded.contains(&name),
                "{target:?} has no seed from {folder}/{name}"
            );
        }
    }
}

#[test]
fn every_case_of_a_file_gives_its_seed() {
    for (target, folder) in [(Target::Ticket, "tickets"), (Target::ManifestPath, "paths")] {
        let cases: usize = vector_files(&vectors_dir().join(folder))
            .unwrap()
            .iter()
            .map(|(_, cases)| cases.len())
            .sum();
        assert_eq!(
            seeds(target, &vectors_dir()).unwrap().len(),
            cases,
            "{target:?}"
        );
    }
}

#[test]
fn a_valid_manifest_also_comes_with_piece_sizes_for_the_unpacker() {
    let seeds = seeds(Target::Manifest, &vectors_dir()).unwrap();
    let valid: usize = vector_files(&vectors_dir().join("manifests"))
        .unwrap()
        .iter()
        .flat_map(|(_, cases)| cases.iter())
        .filter(|case| case["valid"].as_bool() == Some(true))
        .count();
    assert!(valid > 0);
    let with_cuts = seeds
        .iter()
        .filter(|seed| sluice_core::manifest::Manifest::decode(&seed.bytes, limits()).is_err())
        .count();
    assert!(
        with_cuts >= 4 * valid,
        "{with_cuts} inputs that do not decode whole"
    );
}

fn limits() -> sluice_core::manifest::Limits {
    sluice_core::manifest::Limits {
        max_bytes: 1 << 16,
        max_entries: 1000,
    }
}

#[test]
fn the_range_seeds_are_written_here_and_every_one_parses() {
    let seeds = seeds(Target::Range, &vectors_dir()).unwrap();
    assert!(!seeds.is_empty());
    for seed in seeds {
        assert!(seed.file.is_none());
        let text = std::str::from_utf8(&seed.bytes).unwrap();
        assert!(sluice_core::range::ByteRange::parse(text).is_ok(), "{text}");
    }
}

#[test]
fn a_seed_file_is_named_by_its_content_and_a_second_run_adds_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = dir.path().join("corpus");
    run(Target::Ticket, &corpus).unwrap();
    let names: Vec<String> = fs::read_dir(&corpus)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    assert!(!names.is_empty());
    for name in &names {
        let bytes = fs::read(corpus.join(name)).unwrap();
        assert_eq!(name, blake3::hash(&bytes).to_hex().as_str());
    }
    run(Target::Ticket, &corpus).unwrap();
    assert_eq!(fs::read_dir(&corpus).unwrap().count(), names.len());
}

#[test]
fn a_vector_folder_that_is_not_there_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    assert!(seeds(Target::Log, dir.path()).is_err());
}
