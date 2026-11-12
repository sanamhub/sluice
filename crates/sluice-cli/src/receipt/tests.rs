//! The report a verified receipt makes, in the two forms the command line writes.

use ed25519_dalek::SigningKey;
use sluice_core::receipt::{Receipt, commitment};

use super::{Check, Report, hex, text};

fn signed() -> Receipt {
    Receipt::sign(
        &SigningKey::from_bytes(&[7; 32]),
        commitment(&[9; 32]),
        1024,
        1_800_000_000_000,
    )
}

fn report(file: Option<Check>) -> Report {
    let receipt = signed();
    Report {
        commit: hex(receipt.commit()),
        total_len: receipt.total_len(),
        sender_key: hex(receipt.sender_key()),
        utc_ms: receipt.utc_ms(),
        file,
        code: 0,
    }
}

/// The keys are the 64 lower-case hex characters every other id in this program is written in.
#[test]
fn the_report_names_the_commitment_and_the_sender_in_hex() {
    let text = text(&report(None));
    assert!(text.contains(&hex(&commitment(&[9; 32]))), "{text}");
    assert!(text.contains("1024 bytes"), "{text}");
    assert!(text.contains("1800000000000 UTC ms"), "{text}");
    assert!(
        !text.contains("file"),
        "a report with no file names one: {text}"
    );
}

/// A file whose send root is the one the receipt commits to reads as a match, and the exit code
/// stays 0. The line names no root.
#[test]
fn a_matching_file_says_so() {
    let check = Check {
        path: "report.pdf".to_string(),
        matched: true,
        found_len: 1024,
        found_commit: Some(hex(&commitment(&[9; 32]))),
    };
    let text = text(&report(Some(check)));
    assert!(
        text.contains("matches: its length and commitment"),
        "{text}"
    );
    assert!(!text.contains(&hex(&[9; 32])), "{text}");
    assert!(text.starts_with("receipt ok: "), "{text}");
}

/// A file of the wrong length was never read, so its line gives the length and no root to blame.
#[test]
fn a_file_of_another_length_is_not_hashed() {
    let check = Check {
        path: "other.bin".to_string(),
        matched: false,
        found_len: 5,
        found_commit: None,
    };
    let mut report = report(Some(check));
    report.code = 4;
    let text = text(&report);
    assert!(text.starts_with("receipt ok, but other.bin"), "{text}");
    assert!(text.contains("is 5 bytes, so it was not read"), "{text}");
}

/// A file of the right length whose content is another says what commitment that content has.
#[test]
fn a_file_of_the_right_length_names_its_own_commitment() {
    let check = Check {
        path: "other.bin".to_string(),
        matched: false,
        found_len: 5,
        found_commit: Some("ab".repeat(32)),
    };
    let mut report = report(Some(check));
    report.code = 4;
    let text = text(&report);
    assert!(text.contains("is 5 bytes and its commitment is"), "{text}");
    assert!(text.contains(&"ab".repeat(32)), "{text}");
}

/// The hex form is two characters per byte, lower case, and holds no separator.
#[test]
fn hex_is_two_lower_case_characters_per_byte() {
    assert_eq!(hex(&[0, 15, 16, 255]), "000f10ff");
    assert_eq!(hex(&[]), "");
}
