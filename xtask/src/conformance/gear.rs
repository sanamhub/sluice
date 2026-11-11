//! The `gear/` vectors: the table the chunker's hash reads, which SPEC.md section 7 derives from a
//! BLAKE3 label rather than transcribing. A table a reader recomputes cannot be mistyped, and this
//! checks that it is recomputed the one way the section states.

use std::path::Path;

use anyhow::Result;
use serde::Deserialize;
use sluice_delta::gear_entry;

use super::{Report, run_kind};

#[derive(Deserialize)]
struct Case {
    /// The byte the entry belongs to.
    index: u8,
    /// The entry as 16 lower-case hex characters, the value as a little-endian `u64`.
    value: String,
}

pub fn run(dir: &Path, report: &mut Report) -> Result<()> {
    run_kind("gear", dir, report, disagreement)
}

fn disagreement(case: &Case) -> Option<String> {
    let expected = match u64::from_str_radix(&case.value, 16) {
        Ok(value) => value,
        Err(_) => {
            return Some(format!(
                "entry {}: the value is not 16 hex characters",
                case.index
            ));
        }
    };
    let got = gear_entry(case.index);
    if got != expected {
        return Some(format!(
            "entry {} is {got:016x}, and the case says {expected:016x}",
            case.index
        ));
    }
    None
}
