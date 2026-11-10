//! `sluice receipt verify`: what a receipt is worth, and what it is not.

mod common;

use std::fs;

use common::{bare, stderr_text, stdout_text};
use ed25519_dalek::SigningKey;
use serde_json::Value;
use sluice_core::receipt::Receipt;

const CONTENT: &[u8] = b"the content a receipt names";

fn root() -> [u8; 32] {
    *blake3::hash(CONTENT).as_bytes()
}

/// A receipt file for `CONTENT`, and the content beside it.
fn fixture() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let receipt = Receipt::sign(
        &SigningKey::from_bytes(&[7; 32]),
        root(),
        CONTENT.len() as u64,
        1_800_000_000_000,
    );
    let receipt_path = dir.path().join("receipt.txt");
    fs::write(&receipt_path, format!("{}\n", receipt.encode())).unwrap();
    let content_path = dir.path().join("content.bin");
    fs::write(&content_path, CONTENT).unwrap();
    (dir, receipt_path, content_path)
}

#[test]
fn a_signed_receipt_verifies_and_prints_its_fields() {
    let (_dir, receipt, _content) = fixture();
    let assert = bare()
        .args(["receipt", "verify"])
        .arg(&receipt)
        .assert()
        .success();
    let text = stdout_text(&assert);
    assert!(text.starts_with("receipt ok: "), "{text}");
    assert!(text.contains(&*hex(&root())), "{text}");
    assert!(text.contains("1800000000000 UTC ms"), "{text}");
}

/// The same receipt in JSON is one object, with a length no JSON number has to hold.
#[test]
fn json_names_the_same_fields() {
    let (_dir, receipt, _content) = fixture();
    let assert = bare()
        .args(["receipt", "verify", "--json"])
        .arg(&receipt)
        .assert()
        .success();
    let text = stdout_text(&assert);
    let parsed: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(parsed["root"], hex(&root()));
    assert_eq!(parsed["total_len"], CONTENT.len().to_string());
    assert_eq!(parsed["utc_ms"], "1800000000000");
    assert_eq!(parsed["file"], Value::Null);
}

/// A receipt names the sender's key, and a tampered one names a signature that does not hold.
#[test]
fn a_tampered_receipt_is_refused_with_nothing_on_stdout() {
    let (_dir, receipt, _content) = fixture();
    let text = fs::read_to_string(&receipt).unwrap();
    let mut bytes = text.trim().to_string();
    // A character in the signature, not in the padding of the last one: a change there is a
    // signature that does not hold rather than a body that does not decode.
    let at = bytes.len() - 40;
    bytes.replace_range(
        at..at + 1,
        if &bytes[at..at + 1] == "a" { "b" } else { "a" },
    );
    fs::write(&receipt, format!("{bytes}\n")).unwrap();
    let assert = bare()
        .args(["receipt", "verify"])
        .arg(&receipt)
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

/// With the content, the file's root and length are checked, and a match is said.
#[test]
fn a_file_that_is_the_content_matches() {
    let (_dir, receipt, content) = fixture();
    let assert = bare()
        .args(["receipt", "verify"])
        .arg(&receipt)
        .args(["--file"])
        .arg(&content)
        .assert()
        .success();
    assert!(
        stdout_text(&assert).contains("matches the root and the length"),
        "{:?}",
        stdout_text(&assert)
    );
}

/// Another file is not this content: the receipt still verifies, and the command fails with the
/// verification code after saying what the file really is.
#[test]
fn a_file_that_is_other_content_does_not_match() {
    let (dir, receipt, _content) = fixture();
    let other = dir.path().join("other.bin");
    fs::write(&other, b"other bytes").unwrap();
    let assert = bare()
        .args(["receipt", "verify"])
        .arg(&receipt)
        .args(["--file"])
        .arg(&other)
        .assert()
        .code(4);
    let text = stdout_text(&assert);
    assert!(text.starts_with("receipt ok, but "), "{text}");
    assert!(text.contains("is 11 bytes"), "{text}");
    assert!(
        text.contains(&*hex(blake3::hash(b"other bytes").as_bytes())),
        "{text}"
    );
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
