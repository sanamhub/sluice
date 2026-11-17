//! The `alpn` vectors. A case is one position of the listener's registration order, the ALPN there
//! and whether this build serves it. iroh negotiates by that order, so `SPEC.md` section 3 pins it.

use std::path::Path;

use anyhow::Result;
use serde::Deserialize;
use sluice_core::engine::ALPN_REGISTERED;

use super::{Report, run_kind};

#[derive(Deserialize)]
struct Case {
    index: usize,
    alpn: String,
    served: bool,
}

pub fn run(dir: &Path, report: &mut Report) -> Result<()> {
    // The vector must number every position the listener registers and no other, or an ALPN added
    // at the end of the order would leave the spec silently stale.
    if let Some(cases) = listed(dir) {
        let mut indexes: Vec<usize> = cases.iter().map(|case| case.index).collect();
        indexes.sort_unstable();
        let want: Vec<usize> = (0..ALPN_REGISTERED.len()).collect();
        if indexes != want {
            report.lines.push(format!(
                "alpn/  FAIL the vector numbers {indexes:?}, the listener registers {} ALPNs",
                ALPN_REGISTERED.len()
            ));
            report.failed += 1;
        }
    }
    run_kind("alpn", dir, report, disagreement)
}

/// Every case the vector holds, or `None` when a file cannot be read, which [`run_kind`] reports
/// itself.
fn listed(dir: &Path) -> Option<Vec<Case>> {
    let mut cases = Vec::new();
    for file in super::json_files(dir).ok()? {
        cases.extend(super::read_file::<Case>(&file).ok()?.cases);
    }
    Some(cases)
}

fn disagreement(case: &Case) -> Option<String> {
    let Some((alpn, served)) = ALPN_REGISTERED.get(case.index) else {
        return Some(format!("there is no ALPN at position {}", case.index));
    };
    if alpn != &case.alpn.as_bytes() {
        return Some(format!(
            "position {} is {:?}, the vector says {:?}",
            case.index,
            String::from_utf8_lossy(alpn),
            case.alpn
        ));
    }
    if *served != case.served {
        return Some(format!(
            "position {} ({:?}) is served: {served}, the vector says {}",
            case.index, case.alpn, case.served
        ));
    }
    None
}
