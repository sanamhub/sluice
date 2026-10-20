//! `cargo xtask conformance`: runs every vector under `tests/vectors/` against this implementation
//! and exits non-zero if one disagrees. The format is in `tests/vectors/README.md`.

mod paths;
#[cfg(test)]
mod tests;

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};

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
            "paths" => paths::run(&dir, &mut report)?,
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
