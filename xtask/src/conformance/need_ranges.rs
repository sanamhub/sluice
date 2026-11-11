//! The `need-ranges/` vectors: the bytes of a `NeedRanges` message and the sets a requester must
//! refuse. SPEC.md section 7 defines the message and says its ranges never touch.

use std::path::Path;

use anyhow::Result;
use serde::Deserialize;
use sluice_delta::{NeedRanges, Range};

use super::{Report, hex_bytes, run_kind};

#[derive(Deserialize)]
struct Case {
    name: String,
    /// The message as lower-case hex.
    bytes: String,
    valid: bool,
    /// Valid cases. The root, as 64 lower-case hex characters.
    root: Option<String>,
    /// Valid cases. Each range as a start and a length, decimal strings, ascending and never
    /// touching.
    ranges: Option<Vec<(String, String)>>,
}

pub fn run(dir: &Path, report: &mut Report) -> Result<()> {
    run_kind("need-ranges", dir, report, disagreement)
}

fn disagreement(case: &Case) -> Option<String> {
    let name = &case.name;
    let bytes = match hex_bytes(&case.bytes) {
        Ok(bytes) => bytes,
        Err(_) => return Some(format!("{name}: the message is not hex")),
    };
    let decoded = NeedRanges::decode(&bytes);
    if !case.valid {
        return match decoded {
            Ok(_) => Some(format!("{name}: a message the spec refuses was read")),
            Err(_) => None,
        };
    }
    let decoded = match decoded {
        Ok(decoded) => decoded,
        Err(error) => {
            return Some(format!(
                "{name}: a message the spec allows was refused: {error}"
            ));
        }
    };
    let Some(root) = case.root.as_deref().and_then(root_bytes) else {
        return Some(format!("{name}: a valid case has no root"));
    };
    let Some(ranges) = &case.ranges else {
        return Some(format!("{name}: a valid case has no ranges"));
    };
    let wanted: Vec<Range> = {
        let mut wanted = Vec::with_capacity(ranges.len());
        for (start, len) in ranges {
            let (start, len) = match (start.parse::<u64>(), len.parse::<u64>()) {
                (Ok(start), Ok(len)) => (start, len),
                _ => return Some(format!("{name}: a range is not a decimal number")),
            };
            match Range::new(start, len) {
                Ok(range) => wanted.push(range),
                Err(error) => return Some(format!("{name}: a range the spec refuses: {error}")),
            }
        }
        wanted
    };
    let expected = match NeedRanges::new(root, wanted) {
        Ok(expected) => expected,
        Err(error) => return Some(format!("{name}: the ranges are not one set: {error}")),
    };
    if decoded != expected {
        return Some(format!(
            "{name}: the message reads as {decoded:?}, not {expected:?}"
        ));
    }
    if decoded.encode() != bytes {
        return Some(format!(
            "{name}: the message does not encode to its own bytes"
        ));
    }
    None
}

/// The 32 bytes of a root written as 64 hex characters.
fn root_bytes(text: &str) -> Option<[u8; 32]> {
    hex_bytes(text).ok()?.try_into().ok()
}
