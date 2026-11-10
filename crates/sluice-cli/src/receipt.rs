//! `sluice receipt verify FILE [--file PATH]`: checks that a receipt is what its sender signed,
//! and optionally that a file is the content it names.
//!
//! A receipt is only a statement. It proves that the sender offered that content at about that
//! time. It does not prove that anyone received it, that the receiver kept it, or who holds the
//! key that signed it.

use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
};

use clap::{Args, Subcommand};
use serde_json::{Value, json};
use sluice_core::{Error, manifest::path::shown, receipt::Receipt};

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
    /// Check a receipt's signature, and with --file that a file is the content it names
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
    /// A file to hash and check against the root and the length the receipt names
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
    pub root: String,
    pub total_len: u64,
    pub sender_key: String,
    pub utc_ms: u64,
    pub file: Option<Check>,
    /// The exit code: 0, or 4 when a named file is not the content the receipt names.
    pub code: u8,
}

/// One file, hashed and compared with the receipt.
pub struct Check {
    pub path: String,
    pub matched: bool,
    pub found_len: u64,
    pub found_root: String,
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
        root: hex(receipt.root()),
        total_len: receipt.total_len(),
        sender_key: hex(receipt.sender_key()),
        utc_ms: receipt.utc_ms(),
        file,
        code,
    }))
}

/// Reads the receipt as text. A receipt is not a capability, so a link is followed, and only the
/// first [`MOST`] bytes are read: whether a file is one is decided by its first characters.
fn read(path: &Path) -> Result<String, Error> {
    let mut text = String::new();
    File::open(path)
        .and_then(|file| file.take(MOST).read_to_string(&mut text))
        .map_err(|e| Error::Storage(format!("cannot read {}: {e}", path.display())))?;
    Ok(text)
}

/// Hashes the file and compares it with the receipt. A file that is not the right length is not
/// hashed to the end: the length alone decides it.
fn check(path: &Path, receipt: &Receipt) -> Result<Check, Error> {
    if path.is_dir() {
        return Err(Error::Policy(format!(
            "{} is a folder, and a receipt names one file of content",
            path.display()
        )));
    }
    let mut file = File::open(path)
        .map_err(|e| Error::Storage(format!("cannot open {}: {e}", path.display())))?;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = vec![0u8; 64 * 1024];
    let mut len = 0u64;
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|e| Error::Storage(format!("cannot read {}: {e}", path.display())))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        len += read as u64;
    }
    let found = hasher.finalize();
    let matched = len == receipt.total_len() && found.as_bytes() == receipt.root();
    Ok(Check {
        path: shown(&path.display().to_string()),
        matched,
        found_len: len,
        found_root: hex(found.as_bytes()),
    })
}

/// The report as a person reads it: the size and the time first, then the two keys.
pub fn text(report: &Report) -> String {
    let mut out = match &report.file {
        Some(check) if !check.matched => format!(
            "receipt ok, but {} is not the content it names\n",
            check.path
        ),
        _ => format!(
            "receipt ok: {}, signed at {} UTC ms\n",
            bytes_text(report.total_len),
            report.utc_ms
        ),
    };
    out.push_str(&format!("root      {}\n", report.root));
    out.push_str(&format!("sender    {}\n", report.sender_key));
    out.push_str(&format!(
        "length    {} bytes ({})\n",
        report.total_len,
        bytes_text(report.total_len)
    ));
    if let Some(check) = &report.file {
        if check.matched {
            out.push_str(&format!(
                "file      {} matches the root and the length\n",
                check.path
            ));
        } else {
            out.push_str(&format!(
                "file      {} is {} bytes and hashes to {}\n",
                check.path, check.found_len, check.found_root
            ));
        }
    }
    out
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
            "root": report.root,
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
