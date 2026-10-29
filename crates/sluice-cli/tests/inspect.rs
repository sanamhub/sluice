//! `sluice inspect <ticket>`: what a ticket offers, read from its manifest by two real commands,
//! without the content and without writing anything.

mod common;

use std::path::Path;

use common::{assert_no_temporary_store, data, sluice, start, start_in, stdout_of};

/// `sluice inspect` in `dir`, with `SLUICE_HOME` set to `home`, and its output as text.
fn inspect_in(home: &Path, dir: &Path, ticket: &str, args: &[&str]) -> String {
    let mut command = sluice(home);
    command
        .current_dir(dir)
        .args(["inspect", ticket])
        .args(args);
    stdout_of(&mut command)
}

/// A one-file ticket is one line and one name, and the command writes nothing where it ran or keeps
/// anything in the home.
#[test]
fn a_file_ticket_is_listed_with_its_size() {
    let dir = tempfile::tempdir().unwrap();
    let sender_home = dir.path().join("sender-home");
    let home = dir.path().join("inspect-home");
    let file = dir.path().join("one.bin");
    let body = data(150 * 1024, 8);
    std::fs::write(&file, &body).unwrap();
    let (sender, ticket) = start(&sender_home, &file, "1h");
    let landing = dir.path().join("landing");
    std::fs::create_dir(&landing).unwrap();

    let text = inspect_in(&home, &landing, &ticket, &[]);
    assert_eq!(text, "1 file, 150.0 KiB\none.bin\n");
    assert_eq!(
        std::fs::read_dir(&landing).unwrap().count(),
        0,
        "inspect wrote into the folder it ran in"
    );
    // The sender is still serving, so its own store is in its own home; this home is the one the
    // command that only read a manifest used.
    assert_no_temporary_store(&home);
    drop(sender);
}

/// A folder's ticket is a tree: each name is indented under the folder it is in, and a folder's own
/// name ends in a slash.
#[test]
fn a_folder_ticket_is_listed_as_a_tree() {
    let dir = tempfile::tempdir().unwrap();
    let sender_home = dir.path().join("sender-home");
    let home = dir.path().join("inspect-home");
    let tree = dir.path().join("tree");
    std::fs::create_dir_all(tree.join("sub")).unwrap();
    std::fs::write(tree.join("a.txt"), b"aaaa").unwrap();
    std::fs::write(tree.join("sub").join("b.txt"), b"bb").unwrap();
    let (sender, ticket) = start(&sender_home, &tree, "1h");
    let landing = dir.path().join("landing");
    std::fs::create_dir(&landing).unwrap();

    let text = inspect_in(&home, &landing, &ticket, &[]);
    assert_eq!(
        text,
        "2 files, 2 folders, 6 bytes\ntree/\n  a.txt\n  sub/\n    b.txt\n"
    );
    assert_eq!(std::fs::read_dir(&landing).unwrap().count(), 0);
    assert_no_temporary_store(&home);
    drop(sender);
}

/// A folder with more names than a terminal should take is cut at fifty lines, and the line after it
/// says how many were left out; `--all` lists every name.
#[test]
fn a_large_folder_is_cut_at_fifty_lines_unless_every_name_is_asked_for() {
    let dir = tempfile::tempdir().unwrap();
    let sender_home = dir.path().join("sender-home");
    let home = dir.path().join("inspect-home");
    let tree = dir.path().join("big");
    std::fs::create_dir(&tree).unwrap();
    for index in 0..60 {
        std::fs::write(tree.join(format!("{index:02}.bin")), b"x").unwrap();
    }
    let (sender, ticket) = start(&sender_home, &tree, "1h");
    let landing = dir.path().join("landing");
    std::fs::create_dir(&landing).unwrap();

    let cut = inspect_in(&home, &landing, &ticket, &[]);
    let lines: Vec<&str> = cut.lines().collect();
    // The summary, the fifty names the limit allows (the folder's own is one of them), and the line
    // that counts the rest.
    assert_eq!(lines.len(), 1 + 50 + 1, "{cut}");
    assert_eq!(lines[0], "60 files, 1 folder, 60 bytes");
    assert_eq!(lines[1], "big/");
    assert_eq!(
        lines.last().unwrap(),
        &"... and 11 more names (--all lists every one)"
    );

    let every = inspect_in(&home, &landing, &ticket, &["--all"]);
    assert_eq!(every.lines().count(), 1 + 61, "{every}");
    assert!(!every.contains("more names"), "{every}");
    assert_no_temporary_store(&home);
    drop(sender);
}

/// `--json` is one object with the counts, the size and every entry, whatever the terminal limit
/// says.
#[test]
fn json_carries_the_counts_and_every_name() {
    let dir = tempfile::tempdir().unwrap();
    let sender_home = dir.path().join("sender-home");
    let home = dir.path().join("inspect-home");
    let tree = dir.path().join("tree");
    std::fs::create_dir_all(tree.join("sub")).unwrap();
    std::fs::write(tree.join("a.txt"), b"aaaa").unwrap();
    std::fs::write(tree.join("sub").join("b.txt"), b"bb").unwrap();
    let (sender, ticket) = start(&sender_home, &tree, "1h");
    let landing = dir.path().join("landing");
    std::fs::create_dir(&landing).unwrap();

    let text = inspect_in(&home, &landing, &ticket, &["--json"]);
    assert_eq!(text.lines().count(), 1, "{text}");
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(value["files"], 2);
    assert_eq!(value["folders"], 2);
    assert_eq!(value["links"], 0);
    assert_eq!(value["bytes"], 6);
    let entries = value["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 4, "{entries:?}");
    assert_eq!(entries[0]["path"], "tree");
    assert_eq!(entries[0]["kind"], "directory");
    assert_eq!(entries[2]["path"], "tree/sub");
    assert_eq!(entries[3]["path"], "tree/sub/b.txt");
    assert_eq!(entries[3]["kind"], "file");
    assert_eq!(entries[3]["size"], 2);
    assert_no_temporary_store(&home);
    drop(sender);
}

/// A live ticket carries no manifest, so there is nothing to read: it says so rather than waiting
/// for a stream that may not have started.
#[test]
fn a_live_ticket_says_it_has_no_names_yet() {
    let dir = tempfile::tempdir().unwrap();
    let sender_home = dir.path().join("sender-home");
    let home = dir.path().join("inspect-home");
    let file = dir.path().join("live.bin");
    std::fs::write(&file, b"growing").unwrap();
    let (sender, ticket) = start_in(
        dir.path(),
        &sender_home,
        &["--live", "live.bin", "--expires", "1h"],
    );
    let landing = dir.path().join("landing");
    std::fs::create_dir(&landing).unwrap();

    let text = inspect_in(&home, &landing, &ticket, &[]);
    assert_eq!(
        text,
        "a live stream, whose names and size arrive with the stream itself\n"
    );
    let value: serde_json::Value =
        serde_json::from_str(&inspect_in(&home, &landing, &ticket, &["--json"])).unwrap();
    assert_eq!(value, serde_json::json!({ "live": true }));
    drop(sender);
}
