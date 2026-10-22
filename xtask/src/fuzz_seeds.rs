//! `cargo xtask fuzz-seeds <target> <dir>`: the starting corpus of one fuzz target, written from
//! the test vectors.
//!
//! A run seeded from the vectors starts with inputs that already reach every rule of a parser, so a
//! minute in CI goes on new inputs and not on finding the format again. CI and hand runs use this
//! one command, so they start from the same seeds.

use std::fs;
use std::path::Path;

use anyhow::{Context, bail};
use serde_json::Value;

use crate::conformance::{hex_bytes, vectors_dir};

/// A fuzz target, by its name in its `fuzz/Cargo.toml`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Target {
    /// `sluice-core`: ticket text.
    Ticket,
    /// `sluice-core`: manifest bytes, and manifests with piece sizes for the unpacker.
    Manifest,
    /// `sluice-core`: manifest paths.
    #[value(name = "manifest_path")]
    ManifestPath,
    /// `sluice-live`: the records of a live stream after its open.
    Log,
    /// `sluice-core`: byte ranges as a person writes them.
    Range,
}

impl Target {
    /// The folder of `tests/vectors/` the target is seeded from, if it has one.
    fn folder(self) -> Option<&'static str> {
        match self {
            Target::Ticket => Some("tickets"),
            Target::Manifest => Some("manifests"),
            Target::ManifestPath => Some("paths"),
            Target::Log => Some("live"),
            Target::Range => None,
        }
    }
}

/// One seed and the vector file it came from, or `None` for one written here.
pub struct Seed {
    pub file: Option<String>,
    pub bytes: Vec<u8>,
}

/// Ranges in every form `ByteRange::parse` takes, and on and off the block boundaries. No vector
/// file holds ranges: the syntax is the command line's and not the wire's, so `SPEC.md` has none.
const RANGES: [&str; 11] = [
    "4G:4.1G",
    "0:",
    ":512",
    "1K:+1K",
    "16384:32768",
    "16383:16385",
    "39000:",
    "1.5GiB:+100MB",
    "18446744073709551614:",
    "0:+18446744073709551615",
    "0.33333333333333333333K:2kib",
];

pub fn run(target: Target, dir: &Path) -> anyhow::Result<()> {
    let seeds = seeds(target, &vectors_dir())?;
    fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    let mut written = 0;
    for seed in &seeds {
        // Named by content, so a seed that two vectors share is one file.
        let path = dir.join(blake3::hash(&seed.bytes).to_hex().as_str());
        if !path.exists() {
            fs::write(&path, &seed.bytes)
                .with_context(|| format!("cannot write {}", path.display()))?;
            written += 1;
        }
    }
    let files: std::collections::BTreeSet<&str> = seeds
        .iter()
        .filter_map(|seed| seed.file.as_deref())
        .collect();
    println!(
        "{} seeds from {} vector files, {written} new files in {}",
        seeds.len(),
        files.len(),
        dir.display()
    );
    Ok(())
}

/// Every seed of `target`, from the vector files under `vectors`.
pub fn seeds(target: Target, vectors: &Path) -> anyhow::Result<Vec<Seed>> {
    let Some(folder) = target.folder() else {
        return Ok(RANGES
            .iter()
            .map(|text| Seed {
                file: None,
                bytes: text.as_bytes().to_vec(),
            })
            .collect());
    };
    let mut seeds = Vec::new();
    for (name, cases) in vector_files(&vectors.join(folder))? {
        for case in &cases {
            for bytes in from_case(target, case).with_context(|| format!("a case of {name}"))? {
                seeds.push(Seed {
                    file: Some(name.clone()),
                    bytes,
                });
            }
        }
    }
    Ok(seeds)
}

/// The `cases` of every `.json` file in `dir`, by file name, in name order.
pub fn vector_files(dir: &Path) -> anyhow::Result<Vec<(String, Vec<Value>)>> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .with_context(|| format!("cannot list {}", dir.display()))?
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .filter(|name| name.ends_with(".json"))
        .collect();
    names.sort();
    let mut files = Vec::new();
    for name in names {
        let path = dir.join(&name);
        let text =
            fs::read_to_string(&path).with_context(|| format!("cannot read {}", path.display()))?;
        let json: Value = serde_json::from_str(&text)
            .with_context(|| format!("{} is not JSON", path.display()))?;
        let Some(cases) = json["cases"].as_array() else {
            bail!("{} has no cases", path.display());
        };
        files.push((name, cases.clone()));
    }
    Ok(files)
}

/// The inputs one vector case gives a target.
fn from_case(target: Target, case: &Value) -> anyhow::Result<Vec<Vec<u8>>> {
    Ok(match target {
        Target::Ticket => vec![text(case, "ticket")?.into_bytes()],
        Target::ManifestPath => vec![text(case, "path")?.into_bytes()],
        Target::Manifest => manifest(case)?,
        Target::Log => vec![log(case)?],
        Target::Range => Vec::new(),
    })
}

fn text(case: &Value, key: &str) -> anyhow::Result<String> {
    case[key]
        .as_str()
        .map(str::to_string)
        .with_context(|| format!("no {key}"))
}

/// The manifest as it is, and for a valid one the same manifest followed by piece sizes and how
/// many there are, which the target reads as the cuts to feed its packs to the unpacker in.
fn manifest(case: &Value) -> anyhow::Result<Vec<Vec<u8>>> {
    let bytes = hex_bytes(&text(case, "bytes")?).map_err(anyhow::Error::msg)?;
    let mut inputs = vec![bytes.clone()];
    if case["valid"].as_bool() == Some(true) {
        for cuts in [&[0u8, 1, 2][..], &[1], &[3, 0, 7, 255], &[5; 8]] {
            let mut input = bytes.clone();
            input.extend_from_slice(cuts);
            input.push(cuts.len() as u8);
            inputs.push(input);
        }
    }
    Ok(inputs)
}

/// The records after the case's first open, in the target's framing: a tag byte (0 frame,
/// 1 close), a length byte, and up to 255 bytes. The target opens its stream itself, with the key
/// and stream of the vectors, so the records of a case that starts on that open are accepted.
fn log(case: &Value) -> anyhow::Result<Vec<u8>> {
    let steps = case["steps"].as_array().context("no steps")?;
    let mut input = Vec::new();
    for step in steps.iter().skip(1) {
        let record = text(step, "record")?;
        if record == "open" {
            break;
        }
        let bytes = hex_bytes(&text(step, "bytes")?).map_err(anyhow::Error::msg)?;
        let len = bytes.len().min(255);
        input.push(u8::from(record == "close"));
        input.push(len as u8);
        input.extend_from_slice(&bytes[..len]);
    }
    Ok(input)
}

#[cfg(test)]
mod tests;
