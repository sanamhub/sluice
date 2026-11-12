//! The `receipt-requests/` vectors: the bytes a receiver writes to ask for a receipt, and the
//! streams a sender must answer with nothing. SPEC.md section 10 defines the request.

use std::path::Path;

use anyhow::Result;
use serde::Deserialize;
use sluice_core::engine::parse_request;

use super::{Report, hex_bytes, run_kind};

#[derive(Deserialize)]
struct Case {
    name: String,
    /// Everything the asker wrote before it ended its side, as lower-case hex.
    bytes: String,
    valid: bool,
    /// Valid cases. The root, as 64 lower-case hex characters.
    root: Option<String>,
}

pub fn run(dir: &Path, report: &mut Report) -> Result<()> {
    run_kind("receipt-requests", dir, report, disagreement)
}

fn disagreement(case: &Case) -> Option<String> {
    let name = &case.name;
    let Ok(bytes) = hex_bytes(&case.bytes) else {
        return Some(format!("{name}: the stream is not hex"));
    };
    let read = parse_request(&bytes);
    if !case.valid {
        return read
            .is_ok()
            .then(|| format!("{name}: a stream the spec refuses was read as a request"));
    }
    let root = match read {
        Ok(root) => root,
        Err(error) => {
            return Some(format!(
                "{name}: a request the spec allows was refused: {error}"
            ));
        }
    };
    let wanted = case.root.as_deref().and_then(|text| hex_bytes(text).ok());
    (wanted.as_deref() != Some(root.as_slice()))
        .then(|| format!("{name}: the request names another root than the vector says"))
}
