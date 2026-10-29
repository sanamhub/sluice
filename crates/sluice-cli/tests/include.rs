//! Taking part of a folder with two real processes: a send serves a tree, and a receive and an
//! inspection take the entries their patterns name.

mod common;

use std::path::Path;

use common::{recv_in, start_in, stdout_of};

/// A folder with two files in it and one nested, so a pattern has something to leave out.
fn folder(dir: &Path) -> std::path::PathBuf {
    let tree = dir.join("tree");
    std::fs::create_dir_all(tree.join("sub")).unwrap();
    std::fs::write(tree.join("keep.txt"), b"keep").unwrap();
    std::fs::write(tree.join("skip.log"), b"skip").unwrap();
    std::fs::write(tree.join("sub").join("deep.txt"), b"deep").unwrap();
    tree
}

/// A receive with `--include` writes the files the pattern names, with the folders above them, and
/// reports what part of the ticket that was before it starts.
#[test]
fn a_receive_takes_only_what_the_pattern_names() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    folder(dir.path());
    let (sender, ticket) = start_in(dir.path(), &home, &["tree", "--expires", "1h"]);
    let out = dir.path().join("out");
    let result = recv_in(
        dir.path(),
        &home,
        &ticket,
        &["--out", out.to_str().unwrap(), "--include", "tree/**/*.txt"],
    );
    let text = String::from_utf8_lossy(&result.stderr).into_owned();
    assert!(result.status.success(), "{text}");
    assert!(
        text.contains("2 files, 2 folders, 8 bytes of 3 files, 12 bytes"),
        "the planned line does not say what part it takes: {text}"
    );
    assert_eq!(
        std::fs::read(out.join("tree").join("keep.txt")).unwrap(),
        b"keep"
    );
    assert_eq!(
        std::fs::read(out.join("tree").join("sub").join("deep.txt")).unwrap(),
        b"deep"
    );
    assert!(
        !out.join("tree").join("skip.log").exists(),
        "a file no pattern names was written"
    );
    drop(sender);
}

/// `inspect --include` prints the view a receive with the same pattern would write, so a pattern can
/// be tried before anything is downloaded.
#[test]
fn an_inspection_prints_the_same_filtered_view() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    folder(dir.path());
    let (sender, ticket) = start_in(dir.path(), &home, &["tree", "--expires", "1h"]);
    let text = stdout_of(common::bare().env("SLUICE_HOME", &home).args([
        "inspect",
        &ticket,
        "--include",
        "tree/sub/**",
    ]));
    assert!(
        text.contains("1 file, 2 folders, 4 bytes of 3 files, 12 bytes"),
        "{text}"
    );
    assert!(text.contains("tree/\n  sub/\n    deep.txt\n"), "{text}");
    assert!(!text.contains("skip.log"), "{text}");
    drop(sender);
}

/// A pattern that matches nothing is a refusal before anything is written, with the code a policy
/// failure has.
#[test]
fn a_pattern_that_matches_nothing_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    folder(dir.path());
    let (sender, ticket) = start_in(dir.path(), &home, &["tree", "--expires", "1h"]);
    let out = dir.path().join("out");
    let result = recv_in(
        dir.path(),
        &home,
        &ticket,
        &["--out", out.to_str().unwrap(), "--include", "nothing/**"],
    );
    assert_eq!(result.status.code(), Some(6));
    let text = String::from_utf8_lossy(&result.stderr);
    assert!(text.contains("matches nothing"), "{text}");
    let written = std::fs::read_dir(&out)
        .map(|entries| entries.count())
        .unwrap_or(0);
    assert_eq!(
        written, 0,
        "a refused selection wrote into the output folder"
    );
    drop(sender);
}

/// `--include` has nothing to choose from on a ticket that names one file, and says so.
#[test]
fn a_one_file_ticket_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let file = dir.path().join("one.bin");
    std::fs::write(&file, b"one").unwrap();
    let (sender, ticket) = start_in(dir.path(), &home, &["one.bin", "--expires", "1h"]);
    let result = recv_in(
        dir.path(),
        &home,
        &ticket,
        &[
            "--out",
            dir.path().join("out").to_str().unwrap(),
            "--include",
            "**",
        ],
    );
    assert_eq!(result.status.code(), Some(6));
    let text = String::from_utf8_lossy(&result.stderr);
    assert!(text.contains("one file"), "{text}");
    drop(sender);
}

/// A pattern that does not parse is a usage error with the same code, and the message names it.
#[test]
fn a_pattern_that_does_not_parse_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let result = recv_in(
        dir.path(),
        &home,
        "sluice1notaticket",
        &["--include", "a/["],
    );
    assert_eq!(result.status.code(), Some(6));
    let text = String::from_utf8_lossy(&result.stderr);
    assert!(text.contains("a/["), "{text}");
}

/// The JSON planned event carries the whole ticket's totals beside the taken ones, so a script can
/// see what part of the transfer it asked for.
#[test]
fn the_json_planned_event_carries_both_totals() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    folder(dir.path());
    let (sender, ticket) = start_in(dir.path(), &home, &["tree", "--expires", "1h"]);
    let out = dir.path().join("out");
    let result = recv_in(
        dir.path(),
        &home,
        &ticket,
        &[
            "--json",
            "--out",
            out.to_str().unwrap(),
            "--include",
            "tree/keep.txt",
        ],
    );
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout = String::from_utf8_lossy(&result.stdout);
    let planned = stdout
        .lines()
        .find(|line| line.contains("\"event\":\"planned\""))
        .expect("no planned event");
    assert!(planned.contains("\"files\":1"), "{planned}");
    assert!(planned.contains("\"of_files\":3"), "{planned}");
    assert!(planned.contains("\"of_bytes\":12"), "{planned}");
    drop(sender);
}

/// The errors of a receive name the file it could not write; that line must not be lost to the
/// escaping of the line it is in.
#[test]
fn a_failure_is_still_one_readable_line() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    folder(dir.path());
    let (sender, ticket) = start_in(dir.path(), &home, &["tree", "--expires", "1h"]);
    let out = dir.path().join("out");
    std::fs::create_dir(&out).unwrap();
    std::fs::create_dir(out.join("tree")).unwrap();
    std::fs::write(out.join("tree").join("keep.txt"), b"not mine").unwrap();
    let result = recv_in(
        dir.path(),
        &home,
        &ticket,
        &["--out", out.to_str().unwrap(), "--include", "tree/keep.txt"],
    );
    assert_eq!(result.status.code(), Some(6));
    let text = String::from_utf8_lossy(&result.stderr);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(
        lines
            .iter()
            .filter(|line| line.starts_with("sluice: "))
            .count(),
        1,
        "{text}"
    );
    assert!(
        lines
            .last()
            .is_some_and(|line| line.starts_with("sluice: policy denied: ")),
        "{text}"
    );
    assert!(!text.contains('\u{1b}'), "{text}");
    drop(sender);
}
