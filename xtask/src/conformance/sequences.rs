//! The `sequences` vectors: a manifest and the hash sequence a ticket for it must name. A valid
//! case is the exact sequence; an invalid one is what a receiver must refuse.

use std::path::Path;

use anyhow::Result;
use serde::Deserialize;
use sluice_core::manifest::{Limits, Manifest};

use super::manifests::hex_bytes;
use super::{Report, run_kind};

#[derive(Deserialize)]
struct Case {
    name: String,
    /// The manifest as hex.
    manifest: String,
    /// The sequence blob as hex.
    sequence: String,
    valid: bool,
}

pub(crate) fn run(dir: &Path, report: &mut Report) -> Result<()> {
    run_kind("sequences", dir, report, |case: &Case| {
        let manifest = match hex_bytes(&case.manifest).and_then(|bytes| {
            Manifest::decode(&bytes, Limits::default()).map_err(|e| e.to_string())
        }) {
            Ok(manifest) => manifest,
            Err(reason) => return Some(format!("{} manifest: {reason}", case.name)),
        };
        let given = match hex_bytes(&case.sequence) {
            Ok(bytes) => bytes,
            Err(reason) => return Some(format!("{} sequence: {reason}", case.name)),
        };
        let expected = manifest.sequence_bytes();
        if (given == expected) == case.valid {
            None
        } else if case.valid {
            Some(format!(
                "{}: the sequence does not match the manifest",
                case.name
            ))
        } else {
            Some(format!(
                "{}: the sequence was accepted but must be refused",
                case.name
            ))
        }
    })
}
