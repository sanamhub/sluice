use std::collections::BTreeSet;
use std::fs;

use serde_json::{Value, json};

use super::{Target, framing, log, run, seeds, vector_files, vector_name};
use crate::conformance::{hex_bytes, vectors_dir};

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
fn every_distinct_case_of_a_file_gives_one_seed() {
    for (target, folder, key) in [
        (Target::Ticket, "tickets", "ticket"),
        (Target::ManifestPath, "paths", "path"),
    ] {
        let inputs: BTreeSet<String> = vector_files(&vectors_dir().join(folder))
            .unwrap()
            .iter()
            .flat_map(|(_, cases)| cases.iter())
            .map(|case| case[key].as_str().unwrap().to_string())
            .collect();
        let seeds = seeds(target, &vectors_dir()).unwrap();
        let distinct: BTreeSet<&[u8]> = seeds.iter().map(|seed| seed.bytes.as_slice()).collect();
        assert_eq!(seeds.len(), inputs.len(), "{target:?}");
        assert_eq!(distinct.len(), seeds.len(), "{target:?} gave a seed twice");
    }
}

#[test]
fn each_valid_manifest_also_comes_at_least_four_times_with_piece_sizes_after_it() {
    let seeds = seeds(Target::Manifest, &vectors_dir()).unwrap();
    let valid: Vec<Vec<u8>> = vector_files(&vectors_dir().join("manifests"))
        .unwrap()
        .iter()
        .flat_map(|(_, cases)| cases.iter())
        .filter(|case| case["valid"].as_bool() == Some(true))
        .map(|case| hex_bytes(case["bytes"].as_str().unwrap()).unwrap())
        .collect();
    assert!(!valid.is_empty());
    for manifest in &valid {
        let with_cuts = seeds
            .iter()
            .filter(|seed| seed.bytes.len() > manifest.len() && seed.bytes.starts_with(manifest))
            .count();
        assert!(
            with_cuts >= 4,
            "{with_cuts} seeds with cuts after a valid manifest"
        );
    }
}

/// The records of a `live/` case after its open and up to a second one, read from the vector.
fn records_of(case: &Value) -> Vec<(bool, Vec<u8>)> {
    case["steps"].as_array().unwrap()[1..]
        .iter()
        .take_while(|step| step["record"] != "open")
        .map(|step| {
            let bytes = hex_bytes(step["bytes"].as_str().unwrap()).unwrap();
            (step["record"] == "close", bytes)
        })
        .collect()
}

#[test]
fn every_log_seed_reads_back_through_the_targets_framing_as_the_records_it_came_from() {
    let mut cases = 0;
    for (name, file_cases) in vector_files(&vectors_dir().join("live")).unwrap() {
        for case in &file_cases {
            let Some(seed) = log(case).unwrap() else {
                continue;
            };
            let read: Vec<(bool, Vec<u8>)> = framing::records(&seed)
                .map(|(is_close, bytes)| (is_close, bytes.to_vec()))
                .collect();
            assert_eq!(read, records_of(case), "{name}: {}", case["name"]);
            cases += 1;
        }
    }
    assert_eq!(cases, 45);
}

#[test]
fn a_live_case_that_opens_on_another_stream_key_or_segment_size_is_skipped() {
    let mut skipped = Vec::new();
    let mut total = 0;
    for (name, file_cases) in vector_files(&vectors_dir().join("live")).unwrap() {
        for case in &file_cases {
            total += 1;
            if log(case).unwrap().is_none() {
                skipped.push(format!("{name}: {}", case["name"].as_str().unwrap()));
            }
        }
    }
    assert_eq!((total, skipped.len()), (65, 20), "{skipped:#?}");
    assert!(
        skipped.contains(&"clean.json: segments of 64 MiB, the largest segment_bytes".to_string()),
        "{skipped:#?}"
    );
    // Of the 45 cases that give a seed, two give the records of another case, so the target is
    // seeded with 43 files, each written once.
    let seeds = seeds(Target::Log, &vectors_dir()).unwrap();
    let distinct: BTreeSet<&[u8]> = seeds.iter().map(|seed| seed.bytes.as_slice()).collect();
    assert_eq!((seeds.len(), distinct.len()), (43, 43));
    let dir = tempfile::tempdir().unwrap();
    run(Target::Log, dir.path()).unwrap();
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 43);
}

/// The bytes of a `live/` case's step: the target's own open for an open, and `ab` repeated for
/// any other record.
fn step_bytes(record: &str, len: usize) -> String {
    match record {
        "open" => framing::OPEN.iter().map(|b| format!("{b:02x}")).collect(),
        _ => "ab".repeat(len),
    }
}

/// A `live/` case named `name` with these steps, each a record and its length.
fn live_case(name: &str, steps: &[(&str, usize)]) -> Value {
    let steps: Vec<Value> = steps
        .iter()
        .map(|(record, len)| json!({"record": record, "bytes": step_bytes(record, *len)}))
        .collect();
    json!({"name": name, "steps": steps})
}

#[test]
fn a_log_case_that_does_not_start_with_an_open_is_an_error_that_names_it() {
    let error = log(&live_case("starts late", &[("frame", 4), ("open", 4)])).unwrap_err();
    assert!(error.to_string().contains("\"starts late\""), "{error}");
}

#[test]
fn a_log_record_over_255_bytes_is_an_error_that_names_the_case_and_not_a_cut() {
    let fits = log(&live_case("fits", &[("open", 124), ("frame", 255)])).unwrap();
    assert_eq!(fits.map(|seed| seed.len()), Some(2 + 255));
    let error = log(&live_case("too long", &[("open", 124), ("close", 256)])).unwrap_err();
    assert_eq!(
        error.to_string(),
        "case \"too long\" has a close of 256 bytes, and the log target's records are at most \
         255 bytes"
    );
}

/// The rule on the name itself, without a file system, so it runs where APFS cannot hold such a
/// name.
#[test]
fn a_vector_file_name_that_is_not_utf8_is_an_error_and_not_a_skip() {
    #[cfg(unix)]
    let name = {
        use std::os::unix::ffi::OsStringExt;
        std::ffi::OsString::from_vec(b"bad\xff.json".to_vec())
    };
    #[cfg(windows)]
    let name = {
        use std::os::windows::ffi::OsStringExt;
        // A lone surrogate: a name NTFS keeps and UTF-8 cannot hold.
        let units: Vec<u16> = [0xd800].into_iter().chain(".json".encode_utf16()).collect();
        std::ffi::OsString::from_wide(&units)
    };
    let error = vector_name(name, std::path::Path::new("vectors")).unwrap_err();
    assert!(error.to_string().contains("not UTF-8"), "{error}");
}

// APFS refuses to create a name that is not valid UTF-8, so this case cannot be built there. The
// rule itself is checked on the name above.
#[cfg(not(target_os = "macos"))]
#[test]
fn a_vector_file_whose_name_is_not_utf8_is_an_error_and_not_a_skip() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("good.json"), r#"{"cases": []}"#).unwrap();
    #[cfg(unix)]
    let name = {
        use std::os::unix::ffi::OsStringExt;
        std::ffi::OsString::from_vec(b"bad\xff.json".to_vec())
    };
    #[cfg(windows)]
    let name = {
        use std::os::windows::ffi::OsStringExt;
        // A lone surrogate: a name NTFS keeps and UTF-8 cannot hold.
        let units: Vec<u16> = [0xd800].into_iter().chain(".json".encode_utf16()).collect();
        std::ffi::OsString::from_wide(&units)
    };
    fs::write(dir.path().join(name), r#"{"cases": []}"#).unwrap();
    let error = vector_files(dir.path()).unwrap_err();
    assert!(error.to_string().contains("not UTF-8"), "{error}");
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
fn the_relay_config_seeds_are_the_shipped_configs_and_every_iroh_example() {
    let seeds = seeds(Target::RelayConfig, &vectors_dir()).unwrap();
    let files: Vec<String> = seeds.iter().filter_map(|seed| seed.file.clone()).collect();
    assert_eq!(files.len(), 23, "{files:?}");
    assert_eq!(
        files[..3],
        [
            "docker/relay/relay.toml",
            "docker/relay/open.toml",
            "docker/rendezvous/manual.toml"
        ]
    );
    assert!(
        files[3..]
            .iter()
            .all(|f| f.starts_with("docker/guard/testdata/iroh-relay-1.4.0/")),
        "{files:?}"
    );
    let relay = fs::read(vectors_dir().join("../../docker/relay/relay.toml")).unwrap();
    assert_eq!(seeds[0].bytes, relay);
    // The configs one line from a rule come after them, and are all TOML, so each reaches the
    // check.
    let near: Vec<_> = seeds.iter().filter(|seed| seed.file.is_none()).collect();
    assert_eq!(near.len(), 6);
    for seed in near {
        let text = std::str::from_utf8(&seed.bytes).unwrap();
        assert!(text.parse::<toml::Table>().is_ok(), "{text}");
    }
}

/// The differential target takes its first byte as a choice and the rest as the config. Every
/// shipped config starts with `#`, so without a byte in front the rest is a comment cut in two and
/// no rule runs. Each seed comes twice, with `0x00` and with `0x01`, and the rest of each is the
/// config as it was, which parses, so it reaches the guard's check.
#[test]
fn each_differential_seed_comes_twice_with_a_choice_byte_and_reaches_the_guards_check() {
    let configs = seeds(Target::RelayConfig, &vectors_dir()).unwrap();
    let seeds = seeds(Target::RelayDifferential, &vectors_dir()).unwrap();
    assert_eq!(seeds.len(), 2 * configs.len());
    for (config, pair) in configs.iter().zip(seeds.chunks(2)) {
        assert_eq!(pair[0].file, config.file);
        for (choice, seed) in [0u8, 1].into_iter().zip(pair) {
            assert_eq!(seed.bytes[0], choice, "{:?}", seed.file);
            assert_eq!(seed.bytes[1..], config.bytes, "{:?}", seed.file);
            let rest = std::str::from_utf8(&seed.bytes[1..]).unwrap();
            assert!(
                rest.parse::<toml::Table>().is_ok(),
                "{:?} is not TOML",
                seed.file
            );
        }
    }
    // What was wrong: with the config's own first byte taken as the choice, a shipped config is
    // cut in the middle of its first comment and is not TOML.
    let shipped = &configs[0].bytes;
    assert_eq!(shipped[0], b'#');
    let cut = std::str::from_utf8(&shipped[1..]).unwrap();
    assert!(cut.parse::<toml::Table>().is_err());
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
