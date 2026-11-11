//! The `receipts` vectors. A valid case gives the text, the fields it decodes to, a signature that
//! verifies and, when it gives a content root, whether the receipt is about that content. A refused
//! case says which part of it is wrong: the text, or the signature under the key it names.

use std::path::Path;

use anyhow::Result;
use serde::Deserialize;
use sluice_core::receipt::Receipt;

use super::manifests::hex_array;
use super::{Report, run_kind, shown};

#[derive(Deserialize)]
struct Case {
    receipt: String,
    valid: bool,
    /// What the text decodes to. Present when `valid` is true.
    fields: Option<Fields>,
    /// A content root the receipt is checked against, and whether it is about that content.
    content: Option<Content>,
    /// What is wrong with a refused case: `text` or `signature`.
    refused: Option<String>,
}

/// The `fields` object. Lengths are decimal strings, because a JSON number does not hold a `u64` in
/// every language.
#[derive(Deserialize)]
struct Fields {
    commit: String,
    total_len: String,
    sender_key: String,
    utc_ms: String,
}

/// A root and whether the receipt commits to it.
#[derive(Deserialize)]
struct Content {
    root: String,
    matches: bool,
}

pub fn run(dir: &Path, report: &mut Report) -> Result<()> {
    run_kind("receipts", dir, report, disagreement)
}

fn disagreement(case: &Case) -> Option<String> {
    let what = shown(&case.receipt);
    if !case.valid {
        return match (case.refused.as_deref(), Receipt::decode(&case.receipt)) {
            (Some("text"), Ok(_)) => Some(format!("{what}: expected a refused text, got accepted")),
            (Some("text"), Err(_)) => None,
            (Some("signature"), Ok(receipt)) => receipt
                .verify()
                .is_ok()
                .then(|| format!("{what}: expected a refused signature, got one that verifies")),
            (Some("signature"), Err(_)) => Some(format!(
                "{what}: expected a refused signature, got a refused text"
            )),
            (other, _) => Some(format!(
                "{what}: `refused` is `{}`, and only `text` and `signature` are known",
                other.unwrap_or("absent")
            )),
        };
    }
    let Some(fields) = &case.fields else {
        return Some(format!("{what}: a valid case needs `fields`"));
    };
    let receipt = match Receipt::decode(&case.receipt) {
        Ok(receipt) => receipt,
        Err(error) => return Some(format!("{what}: expected accepted, got {error}")),
    };
    if let Err(error) = receipt.verify() {
        return Some(format!("{what}: a good receipt did not verify: {error}"));
    }
    if receipt.encode() != case.receipt {
        return Some(format!("{what}: the encoder writes another text"));
    }
    let got = Fields {
        commit: hex(receipt.commit()),
        total_len: receipt.total_len().to_string(),
        sender_key: hex(receipt.sender_key()),
        utc_ms: receipt.utc_ms().to_string(),
    };
    for (name, they, ours) in [
        ("commit", &fields.commit, &got.commit),
        ("total_len", &fields.total_len, &got.total_len),
        ("sender_key", &fields.sender_key, &got.sender_key),
        ("utc_ms", &fields.utc_ms, &got.utc_ms),
    ] {
        if they != ours {
            return Some(format!(
                "{what}: {name} is {they}, and the receipt says {ours}"
            ));
        }
    }
    if let Some(content) = &case.content {
        let root = match hex_array(&content.root) {
            Ok(root) => root,
            Err(error) => return Some(format!("{what}: content root: {error}")),
        };
        if receipt.is_of(&root) != content.matches {
            return Some(format!(
                "{what}: the receipt is {} about the content with that root",
                if content.matches { "not" } else { "" }
            ));
        }
    }
    None
}

/// 64 lower-case hex characters, as a ticket and a receipt are compared in.
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
