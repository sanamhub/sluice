//! `cargo xtask conformance`: runs every vector under `tests/vectors/` against this implementation
//! and exits non-zero if one disagrees. The format is in `tests/vectors/README.md`.

mod live;
#[cfg(test)]
mod live_tests;
mod manifests;
mod packs;
mod paths;
#[cfg(test)]
mod tests;
mod tickets;

pub(crate) use manifests::hex_bytes;

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, de::DeserializeOwned};

/// What one run found, kept apart from the printing so the tests can read it.
#[derive(Debug, Default)]
pub struct Report {
    /// One line per file or skipped directory, each followed by its failing cases.
    pub lines: Vec<String>,
    /// Cases that agreed with the implementation, per kind.
    pub passed: Vec<(String, usize)>,
    /// Cases that disagreed, plus files that could not be read.
    pub failed: usize,
    /// Directories with no runner yet.
    pub skipped: Vec<String>,
}

#[cfg(test)]
impl Report {
    /// How many cases of `kind` passed.
    pub fn passed_in(&self, kind: &str) -> usize {
        self.passed
            .iter()
            .filter(|(k, _)| k == kind)
            .map(|(_, n)| n)
            .sum()
    }
}

/// Where the vectors live, next to the workspace's `Cargo.toml`.
pub fn vectors_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("tests")
        .join("vectors")
}

pub fn run() -> Result<()> {
    let report = check(&vectors_dir())?;
    for line in &report.lines {
        println!("{line}");
    }
    let passed: usize = report.passed.iter().map(|(_, n)| n).sum();
    println!(
        "{passed} cases passed, {} failed, {} directories skipped",
        report.failed,
        report.skipped.len()
    );
    ensure!(report.failed == 0, "{} conformance failures", report.failed);
    Ok(())
}

/// Runs every kind it knows. A directory it does not know is listed as skipped and never counted
/// as passed, so a vector kind that gains files before it gains a runner cannot look covered.
pub fn check(root: &Path) -> Result<Report> {
    let mut report = Report::default();
    let mut dirs: Vec<PathBuf> = fs::read_dir(root)
        .with_context(|| format!("cannot read {}", root.display()))?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| path.is_dir())
        .collect();
    dirs.sort();
    for dir in dirs {
        let kind = dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        match kind.as_str() {
            "live" => live::run(&dir, &mut report)?,
            "manifests" => manifests::run(&dir, &mut report)?,
            "packs" => packs::run(&dir, &mut report)?,
            "paths" => paths::run(&dir, &mut report)?,
            "tickets" => tickets::run(&dir, &mut report)?,
            _ => {
                report
                    .lines
                    .push(format!("{kind}/  skipped (no runner for this kind yet)"));
                report.skipped.push(kind);
            }
        }
    }
    Ok(report)
}

/// The JSON files of one vector directory, in name order.
fn json_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut files: Vec<PathBuf> = fs::read_dir(dir)
        .with_context(|| format!("cannot read {}", dir.display()))?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| path.extension().is_some_and(|x| x == "json"))
        .collect();
    files.sort();
    Ok(files)
}

/// One vector file: what its cases have in common, and the cases.
#[derive(Deserialize)]
struct VectorFile<C> {
    description: String,
    cases: Vec<C>,
}

/// Runs `disagreement` on every case of every file in `dir`. It returns what a case got wrong, or
/// `None` if the implementation agrees with it. The description of the file follows each failure,
/// so whoever reads the output knows what the case was meant to show.
fn run_kind<C: DeserializeOwned>(
    kind: &str,
    dir: &Path,
    report: &mut Report,
    disagreement: impl Fn(&C) -> Option<String>,
) -> Result<()> {
    let files = json_files(dir)?;
    if files.is_empty() {
        // A directory that passes because it holds nothing would hide a vector set that went
        // missing.
        report.lines.push(format!("{kind}/  FAIL no vector files"));
        report.failed += 1;
        return Ok(());
    }
    for file in files {
        let name = file
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        match read_file::<C>(&file) {
            Ok(vectors) => {
                let failures: Vec<String> = vectors
                    .cases
                    .iter()
                    .filter_map(&disagreement)
                    .map(|what| format!("FAIL {what}. {}", vectors.description))
                    .collect();
                let total = vectors.cases.len();
                let passed = total - failures.len();
                report
                    .lines
                    .push(format!("{kind}/{name}  {passed} of {total} passed"));
                report
                    .lines
                    .extend(failures.iter().map(|f| format!("  {f}")));
                report.passed.push((kind.to_string(), passed));
                report.failed += failures.len();
            }
            Err(reason) => {
                report.lines.push(format!("{kind}/{name}  FAIL {reason}"));
                report.failed += 1;
            }
        }
    }
    Ok(())
}

fn read_file<C: DeserializeOwned>(file: &Path) -> Result<VectorFile<C>, String> {
    let text = fs::read_to_string(file).map_err(|e| format!("cannot read: {e}"))?;
    let vectors: VectorFile<C> =
        serde_json::from_str(&text).map_err(|e| format!("not a vector file: {e}"))?;
    if vectors.cases.is_empty() {
        return Err("has no cases".to_string());
    }
    Ok(vectors)
}

/// How much of a vector's text a failure line prints. Some are 4 KiB long.
const SHOWN_CHARS: usize = 60;

/// The first `SHOWN_CHARS` characters in quotes, and the length in bytes.
fn shown(text: &str) -> String {
    let head: String = text.chars().take(SHOWN_CHARS).collect();
    let cut = if text.chars().count() > SHOWN_CHARS {
        "..."
    } else {
        ""
    };
    format!("{head:?}{cut} ({} bytes)", text.len())
}
