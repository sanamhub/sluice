//! The `access` vectors. A case is one `iroh-blobs` event and the mode a provider registers for it,
//! and a second mode for a send that has a rate limit. `SPEC.md` section 5 states the mask.

use std::path::Path;

use anyhow::Result;
use serde::Deserialize;
use sluice_core::engine::access_modes;

use super::{Report, run_kind};

#[derive(Deserialize)]
struct Case {
    event: String,
    mode: String,
    /// The mode when the send has a rate limit. Absent means the same as `mode`.
    #[serde(default)]
    mode_when_limited: Option<String>,
}

pub fn run(dir: &Path, report: &mut Report) -> Result<()> {
    // The vector must name every event of the mask and no other, or a new event added to the mask
    // would leave the spec silently stale.
    if let Some(events) = listed(dir) {
        let want = access_modes()
            .iter()
            .map(|(event, _, _)| *event)
            .collect::<Vec<_>>();
        let got = events.iter().map(String::as_str).collect::<Vec<_>>();
        if got != want {
            report.lines.push(format!(
                "access/  FAIL the vector names {got:?}, the mask has {want:?}"
            ));
            report.failed += 1;
        }
    }
    run_kind("access", dir, report, disagreement)
}

/// Every event the vector names, in file order, or `None` when a file cannot be read, which
/// [`run_kind`] reports itself.
fn listed(dir: &Path) -> Option<Vec<String>> {
    let mut events = Vec::new();
    for file in super::json_files(dir).ok()? {
        let vectors = super::read_file::<Case>(&file).ok()?;
        events.extend(vectors.cases.into_iter().map(|case| case.event));
    }
    Some(events)
}

fn disagreement(case: &Case) -> Option<String> {
    let modes = access_modes();
    let Some((_, mode, limited)) = modes.iter().find(|(event, _, _)| *event == case.event) else {
        return Some(format!("{:?} is not an event of the mask", case.event));
    };
    if *mode != case.mode {
        return Some(format!(
            "{:?} without a rate limit is {mode}, the vector says {}",
            case.event, case.mode
        ));
    }
    let want = case.mode_when_limited.as_deref().unwrap_or(&case.mode);
    if *limited != want {
        return Some(format!(
            "{:?} with a rate limit is {limited}, the vector says {want}",
            case.event
        ));
    }
    None
}
