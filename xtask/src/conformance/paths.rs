//! The `paths` vectors: each case is a manifest path and whether a receiver accepts it.

use std::fs;
use std::path::Path;

use anyhow::Result;
use serde::Deserialize;
use sluice_core::manifest::path::ManifestPath;

use super::{Report, json_files};

/// How much of a path a failure line prints. Some vectors are 4 KiB long.
const SHOWN_CHARS: usize = 60;

#[derive(Deserialize)]
struct VectorFile {
    description: String,
    cases: Vec<Case>,
}

/// `rule` is not read here: the format says a runner checks `valid` only.
#[derive(Deserialize)]
struct Case {
    path: String,
    valid: bool,
}

pub fn run(dir: &Path, report: &mut Report) -> Result<()> {
    let files = json_files(dir)?;
    if files.is_empty() {
        // A directory that passes because it holds nothing would hide a vector set that went missing.
        report
            .lines
            .push("paths/  FAIL no vector files".to_string());
        report.failed += 1;
        return Ok(());
    }
    for file in files {
        let name = file
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        match read(&file) {
            Ok(vectors) => check_file(&name, &vectors, report),
            Err(reason) => {
                report.lines.push(format!("paths/{name}  FAIL {reason}"));
                report.failed += 1;
            }
        }
    }
    Ok(())
}

fn read(file: &Path) -> Result<VectorFile, String> {
    let text = fs::read_to_string(file).map_err(|e| format!("cannot read: {e}"))?;
    let vectors: VectorFile =
        serde_json::from_str(&text).map_err(|e| format!("not a vector file: {e}"))?;
    if vectors.cases.is_empty() {
        return Err("has no cases".to_string());
    }
    Ok(vectors)
}

fn check_file(name: &str, vectors: &VectorFile, report: &mut Report) {
    let failures: Vec<String> = vectors
        .cases
        .iter()
        .filter_map(|case| {
            let accepted = ManifestPath::parse(&case.path).is_ok();
            (accepted != case.valid).then(|| describe(case, &vectors.description))
        })
        .collect();
    let passed = vectors.cases.len() - failures.len();
    report.lines.push(format!(
        "paths/{name}  {passed} of {} passed",
        vectors.cases.len()
    ));
    report
        .lines
        .extend(failures.iter().map(|f| format!("  {f}")));
    report.passed.push(("paths".to_string(), passed));
    report.failed += failures.len();
}

fn describe(case: &Case, description: &str) -> String {
    let shown: String = case.path.chars().take(SHOWN_CHARS).collect();
    let cut = if case.path.chars().count() > SHOWN_CHARS {
        "..."
    } else {
        ""
    };
    let (expected, got) = if case.valid {
        ("accepted", "rejected")
    } else {
        ("rejected", "accepted")
    };
    format!(
        "FAIL {shown:?}{cut} ({} bytes): expected {expected}, got {got}. {description}",
        case.path.len()
    )
}
