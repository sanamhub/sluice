//! The `paths` vectors: each case is a manifest path and whether a receiver accepts it.

use std::path::Path;

use anyhow::Result;
use serde::Deserialize;
use sluice_core::manifest::path::ManifestPath;

use super::{Report, run_kind, shown};

/// `rule` is not read here: the format says a runner checks `valid` only.
#[derive(Deserialize)]
struct Case {
    path: String,
    valid: bool,
}

pub fn run(dir: &Path, report: &mut Report) -> Result<()> {
    run_kind("paths", dir, report, disagreement)
}

fn disagreement(case: &Case) -> Option<String> {
    let accepted = ManifestPath::parse(&case.path).is_ok();
    if accepted == case.valid {
        return None;
    }
    let (expected, got) = if case.valid {
        ("accepted", "rejected")
    } else {
        ("rejected", "accepted")
    };
    Some(format!(
        "{}: expected {expected}, got {got}",
        shown(&case.path)
    ))
}
