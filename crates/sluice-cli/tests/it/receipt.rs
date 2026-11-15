//! `sluice receipt verify`: what a receipt is worth, and what it is not.

use std::fs;

use crate::common::{bare, start, start_in, stderr_text, stdout_text};
use ed25519_dalek::SigningKey;
use serde_json::Value;
use sluice_core::{
    receipt::{Receipt, commitment},
    ticket::{Body, Ticket},
};

const CONTENT: &[u8] = b"the content a receipt names";

fn key() -> SigningKey {
    SigningKey::from_bytes(&[7; 32])
}

/// A receipt for content whose root a send names. The signing key is the test's: a receipt's
/// signature is checked under the key the receipt itself names.
fn receipt_for(root: [u8; 32], len: u64) -> Receipt {
    Receipt::sign(&key(), commitment(&root), len, 1_800_000_000_000)
}

fn write_receipt(dir: &std::path::Path, receipt: &Receipt) -> std::path::PathBuf {
    let path = dir.join("receipt.txt");
    fs::write(&path, format!("{}\n", receipt.encode())).unwrap();
    path
}

/// A ticket's static root and size, which is what a receipt for that content names.
fn of_ticket(text: &str) -> ([u8; 32], u64) {
    let ticket = Ticket::decode(text).unwrap();
    match ticket.body() {
        Body::Static { root, size, .. } => (*root, *size),
        Body::Live { .. } => panic!("a live ticket names no manifest"),
    }
}

#[test]
fn a_signed_receipt_verifies_and_prints_its_commitment() {
    let dir = tempfile::tempdir().unwrap();
    let receipt = receipt_for([9; 32], 1024);
    let path = write_receipt(dir.path(), &receipt);
    let assert = bare()
        .args(["receipt", "verify"])
        .arg(&path)
        .assert()
        .success();
    let text = stdout_text(&assert);
    assert!(text.starts_with("receipt ok: "), "{text}");
    assert!(
        text.contains(&hex(&commitment(&[9; 32]))),
        "the commitment is printed: {text}"
    );
    assert!(text.contains("1800000000000 UTC ms"), "{text}");
    assert!(!text.contains(&hex(&[9; 32])), "{text}");
}

/// The same receipt in JSON is one object, with a length no JSON number has to hold.
#[test]
fn json_names_the_same_fields() {
    let dir = tempfile::tempdir().unwrap();
    let receipt = receipt_for([9; 32], 1024);
    let path = write_receipt(dir.path(), &receipt);
    let assert = bare()
        .args(["receipt", "verify", "--json"])
        .arg(&path)
        .assert()
        .success();
    let text = stdout_text(&assert);
    let parsed: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(parsed["commit"], hex(&commitment(&[9; 32])));
    assert_eq!(parsed["total_len"], "1024");
    assert_eq!(parsed["utc_ms"], "1800000000000");
    assert_eq!(parsed["file"], Value::Null);
}

/// A receipt names the sender's key, and a tampered one names a signature that does not hold.
#[test]
fn a_tampered_receipt_is_refused_with_nothing_on_stdout() {
    let dir = tempfile::tempdir().unwrap();
    let receipt = receipt_for([9; 32], 1024);
    let path = write_receipt(dir.path(), &receipt);
    let text = fs::read_to_string(&path).unwrap();
    let mut bytes = text.trim().to_string();
    // A character in the signature, not in the padding of the last one: a change there is a
    // signature that does not hold rather than a body that does not decode.
    let at = bytes.len() - 40;
    bytes.replace_range(
        at..at + 1,
        if &bytes[at..at + 1] == "a" { "b" } else { "a" },
    );
    fs::write(&path, format!("{bytes}\n")).unwrap();
    let assert = bare()
        .args(["receipt", "verify"])
        .arg(&path)
        .assert()
        .code(4);
    assert_eq!(stdout_text(&assert), "");
    let stderr = stderr_text(&assert);
    assert!(stderr.contains("does not verify"), "{stderr:?}");
}

/// A file that is not a receipt is a policy refusal, and its text is never quoted.
#[test]
fn a_file_that_is_not_a_receipt_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("receipt.txt");
    fs::write(&path, "sluice1not-a-receipt\n").unwrap();
    let assert = bare()
        .args(["receipt", "verify"])
        .arg(&path)
        .assert()
        .code(6);
    assert_eq!(stdout_text(&assert), "");
    let stderr = stderr_text(&assert);
    assert!(stderr.contains("receipt does not start"), "{stderr:?}");
    assert!(!stderr.contains("not-a-receipt"), "{stderr:?}");
}

/// A receipt is text. Bytes that are not UTF-8 are a policy error rather than a read failure or a
/// panic, and the message names the file and not its bytes.
#[test]
fn a_file_that_is_not_text_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("receipt.bin");
    fs::write(&path, [0xff, 0xfe, 0x00, 0x81]).unwrap();
    let assert = bare()
        .args(["receipt", "verify"])
        .arg(&path)
        .assert()
        .code(6);
    let stderr = stderr_text(&assert);
    assert!(stderr.contains("not UTF-8 text"), "{stderr:?}");
    assert!(stderr.contains("receipt.bin"), "{stderr:?}");
}

/// One line ending is allowed, and a `\r` is one only together with the `\n` after it.
#[test]
fn a_lone_carriage_return_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let receipt = receipt_for([9; 32], 1024);
    let path = write_receipt(dir.path(), &receipt);
    let text = fs::read_to_string(&path).unwrap();
    fs::write(&path, text.trim_end().to_string() + "\r").unwrap();
    let assert = bare()
        .args(["receipt", "verify"])
        .arg(&path)
        .assert()
        .code(6);
    let stderr = stderr_text(&assert);
    assert!(stderr.contains("no `\\n` follows"), "{stderr:?}");
}

/// A receipt from a real send, checked against the file that send named: the root the verify
/// command rebuilds is the root the ticket carries.
#[test]
fn a_receipt_from_a_real_send_matches_its_file() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let source = dir.path().join("note.txt");
    fs::write(&source, CONTENT).unwrap();
    let (_sender, ticket) = start(&home, &source, "1h");
    let (root, size) = of_ticket(&ticket);
    let path = write_receipt(dir.path(), &receipt_for(root, size));
    let assert = bare()
        .args(["receipt", "verify"])
        .arg(&path)
        .args(["--file"])
        .arg(&source)
        .assert()
        .success();
    let text = stdout_text(&assert);
    assert!(
        text.contains("matches: its length and commitment"),
        "{text}"
    );
    // The root of the content is a ticket body, so the report never carries it.
    assert!(!text.contains(&hex(&root)), "{text}");
}

/// The JSON form of a file check carries the commitment of what it found, and never the root:
/// the root, the length and the receipt's sender key together are a ticket body.
#[test]
fn json_for_a_file_check_never_carries_the_root() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let source = dir.path().join("note.txt");
    fs::write(&source, CONTENT).unwrap();
    let (_sender, ticket) = start(&home, &source, "1h");
    let (root, size) = of_ticket(&ticket);
    let path = write_receipt(dir.path(), &receipt_for(root, size));
    let assert = bare()
        .args(["receipt", "verify", "--json"])
        .arg(&path)
        .args(["--file"])
        .arg(&source)
        .assert()
        .success();
    let text = stdout_text(&assert);
    assert!(!text.contains(&hex(&root)), "{text}");
    let parsed: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(parsed["file"]["matched"], true);
    assert_eq!(
        parsed["file"]["commit"], parsed["commit"],
        "a match commits to the receipt's own commitment"
    );
    assert!(parsed["file"].get("root").is_none(), "{text}");
}

/// A receipt whose commit is the raw root of the content is not the receipt a send makes, and a
/// file check refuses it: the send root is a hash-sequence root, not the bytes' root.
#[test]
fn a_commit_that_is_the_raw_root_does_not_match() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let source = dir.path().join("note.txt");
    fs::write(&source, CONTENT).unwrap();
    let (_sender, ticket) = start(&home, &source, "1h");
    let (root, _size) = of_ticket(&ticket);
    let raw = *blake3::hash(CONTENT).as_bytes();
    assert_ne!(raw, root);
    let path = write_receipt(dir.path(), &receipt_for(raw, CONTENT.len() as u64));
    let assert = bare()
        .args(["receipt", "verify"])
        .arg(&path)
        .args(["--file"])
        .arg(&source)
        .assert()
        .code(4);
    let text = stdout_text(&assert);
    assert!(text.starts_with("receipt ok, but "), "{text}");
    assert!(text.contains("and its commitment is"), "{text}");
}

/// The same file after an edit is other content, and the check says what it found.
#[test]
fn a_changed_file_does_not_match() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let source = dir.path().join("note.txt");
    fs::write(&source, CONTENT).unwrap();
    let (_sender, ticket) = start(&home, &source, "1h");
    let (root, size) = of_ticket(&ticket);
    let path = write_receipt(dir.path(), &receipt_for(root, size));
    fs::write(&source, b"the content a receipt nameZ").unwrap();
    let assert = bare()
        .args(["receipt", "verify"])
        .arg(&path)
        .args(["--file"])
        .arg(&source)
        .assert()
        .code(4);
    let text = stdout_text(&assert);
    assert!(text.contains("and its commitment is"), "{text}");
}

/// A file of another length is not hashed: the length is read from the metadata first, and the
/// command says the file was not read rather than naming a root it never computed.
#[test]
fn a_file_of_the_wrong_length_is_not_read() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let source = dir.path().join("note.txt");
    fs::write(&source, CONTENT).unwrap();
    let (_sender, ticket) = start(&home, &source, "1h");
    let (root, size) = of_ticket(&ticket);
    let path = write_receipt(dir.path(), &receipt_for(root, size));
    let longer = dir.path().join("longer.txt");
    fs::write(&longer, [CONTENT, b" and more"].concat()).unwrap();
    let assert = bare()
        .args(["receipt", "verify"])
        .arg(&path)
        .args(["--file"])
        .arg(&longer)
        .assert()
        .code(4);
    let text = stdout_text(&assert);
    assert!(text.contains("so it was not read"), "{text}");
    assert!(!text.contains("commitment is"), "{text}");
}

/// A folder send names a root the same way, and the check rebuilds it from the folder.
#[test]
fn a_folder_from_a_real_send_matches() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let folder = dir.path().join("tree");
    fs::create_dir_all(folder.join("sub")).unwrap();
    fs::write(folder.join("a.txt"), b"aaaa").unwrap();
    fs::write(folder.join("sub").join("b.txt"), b"bb").unwrap();
    let (_sender, ticket) = start_in(dir.path(), &home, &["tree", "--expires", "1h"]);
    let (root, size) = of_ticket(&ticket);
    let path = write_receipt(dir.path(), &receipt_for(root, size));
    let assert = bare()
        .args(["receipt", "verify"])
        .arg(&path)
        .args(["--file"])
        .arg(&folder)
        .assert()
        .success();
    assert!(
        stdout_text(&assert).contains("matches: its length and commitment"),
        "{}",
        stdout_text(&assert)
    );
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
