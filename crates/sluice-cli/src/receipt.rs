//! `sluice receipt verify FILE [--file PATH]`: checks that a receipt is what its sender signed,
//! and optionally that a file or folder is the content it names.
//!
//! A receipt is only a statement. It proves that the sender offered that content at about that
//! time. It does not prove that anyone received it, that the receiver kept it, or who holds the
//! key that signed it.
//!
//! The receipt holds a commitment to the content's root, never the root, so it cannot be turned
//! into a ticket. A file is checked by rebuilding the root a send of it would name, through the
//! same manifest code the send uses, and committing that root.

use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
};

use clap::{Args, Subcommand};
use serde_json::{Value, json};
use sluice_core::{
    Error,
    engine::{PathRoot, root_of_path},
    manifest::path::shown,
    receipt::Receipt,
};

use crate::output::{Outcome, bytes_text};

/// The most of a receipt file that is read: a receipt is 232 characters of text and the rest of a
/// file is not one.
const MOST: u64 = 4096;

#[derive(Args)]
pub struct ReceiptArgs {
    #[command(subcommand)]
    pub command: ReceiptCommand,
}

#[derive(Subcommand)]
pub enum ReceiptCommand {
    /// Check a receipt, and with --file that a file or folder is the content it names
    ///
    /// A receipt says the sender offered this content at about this time. It does not say that
    /// anyone received it, that it is still held anywhere, or who holds the key that signed it.
    Verify(VerifyArgs),
}

#[derive(Args)]
pub struct VerifyArgs {
    /// A file holding the receipt
    #[arg(value_name = "FILE")]
    pub receipt: PathBuf,
    /// A file or folder to hash and compare with the receipt's commitment and length
    #[arg(long, value_name = "PATH")]
    pub file: Option<PathBuf>,
}

pub fn run(args: &ReceiptArgs) -> Result<Outcome, Error> {
    match &args.command {
        ReceiptCommand::Verify(verify) => verify_one(verify),
    }
}

/// What a verified receipt says, and what a file had to say about it.
pub struct Report {
    pub commit: String,
    pub total_len: u64,
    pub sender_key: String,
    pub utc_ms: u64,
    pub file: Option<Check>,
    /// The exit code: 0, or 4 when a named file is not the content the receipt names.
    pub code: u8,
}

/// One file or folder, checked against the receipt.
pub struct Check {
    pub path: String,
    pub matched: bool,
    pub found_len: u64,
    /// The root a send of the path names, or `None` when the length was already wrong and the path
    /// was not read.
    pub found_root: Option<String>,
}

fn verify_one(args: &VerifyArgs) -> Result<Outcome, Error> {
    let text = read(&args.receipt)?;
    let receipt = Receipt::decode(&text)?;
    receipt.verify()?;
    let file = args
        .file
        .as_deref()
        .map(|path| check(path, &receipt))
        .transpose()?;
    let code = match &file {
        Some(check) if !check.matched => 4,
        _ => 0,
    };
    Ok(Outcome::Receipt(Report {
        commit: hex(receipt.commit()),
        total_len: receipt.total_len(),
        sender_key: hex(receipt.sender_key()),
        utc_ms: receipt.utc_ms(),
        file,
        code,
    }))
}

/// Reads the receipt as text. A receipt is not a capability, so a link is followed, and only the
/// first [`MOST`] bytes are read: whether a file is one is decided by its first characters. A file
/// that is not UTF-8 text, or whose text is not a receipt, is refused as a policy error, and the
/// message names the path and never what the file holds.
fn read(path: &Path) -> Result<String, Error> {
    let shown = shown(&path.display().to_string());
    let mut bytes = Vec::new();
    File::open(path)
        .and_then(|file| file.take(MOST).read_to_end(&mut bytes))
        .map_err(|e| Error::Storage(format!("cannot read {shown}: {e}")))?;
    String::from_utf8(bytes)
        .map_err(|_| Error::Policy(format!("{shown} is not UTF-8 text, so it is not a receipt")))
}

/// Rebuilds the root a send of `path` names and compares it with the receipt. The length is read
/// from the metadata first, so a file of the wrong length is not hashed at all.
fn check(path: &Path, receipt: &Receipt) -> Result<Check, Error> {
    let shown = shown(&path.display().to_string());
    let meta =
        std::fs::metadata(path).map_err(|e| Error::Storage(format!("cannot read {shown}: {e}")))?;
    if meta.is_file() && meta.len() != receipt.total_len() {
        return Ok(Check {
            path: shown,
            matched: false,
            found_len: meta.len(),
            found_root: None,
        });
    }
    let PathRoot {
        root, total_len, ..
    } = root_of_path(path)?;
    Ok(Check {
        path: shown,
        matched: receipt.total_len() == total_len && receipt.is_of(&root),
        found_len: total_len,
        found_root: Some(hex(&root)),
    })
}

/// The report as a person reads it: the size and the time first, then the two keys.
pub fn text(report: &Report) -> String {
    let mut out = match &report.file {
        Some(check) if !check.matched => {
            format!(
                "receipt ok, but {} is not the content it names\n",
                check.path
            )
        }
        _ => format!(
            "receipt ok: {}, signed at {} UTC ms\n",
            bytes_text(report.total_len),
            report.utc_ms
        ),
    };
    out.push_str(&format!("commit    {}\n", report.commit));
    out.push_str(&format!("sender    {}\n", report.sender_key));
    out.push_str(&format!(
        "length    {} bytes ({})\n",
        report.total_len,
        bytes_text(report.total_len)
    ));
    if let Some(check) = &report.file {
        out.push_str(&describe(check));
    }
    out
}

/// The line about the named file, which differs by how far the check got.
fn describe(check: &Check) -> String {
    if check.matched {
        return format!(
            "file      {} matches: its send root is {}\n",
            check.path,
            check.found_root.as_deref().unwrap_or_default()
        );
    }
    match &check.found_root {
        Some(root) => format!(
            "file      {} is {} bytes and its send root is {}\n",
            check.path, check.found_len, root
        ),
        None => format!(
            "file      {} is {} bytes, so it was not read\n",
            check.path, check.found_len
        ),
    }
}

/// The report as one JSON object. A length is a decimal string, as the ticket's own fields are,
/// because a JSON number does not hold a `u64` in every language.
pub fn json(report: &Report) -> String {
    let file: Value = match &report.file {
        Some(check) => json!({
            "path": check.path,
            "matched": check.matched,
            "length": check.found_len.to_string(),
            "root": check.found_root,
        }),
        None => Value::Null,
    };
    format!(
        "{}\n",
        json!({
            "commit": report.commit,
            "total_len": report.total_len.to_string(),
            "sender_key": report.sender_key,
            "utc_ms": report.utc_ms.to_string(),
            "file": file,
        })
    )
}

/// 64 lower-case hex characters, as `sluice id` writes an endpoint id.
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests;
