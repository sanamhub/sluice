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

// The `log` target's own framing and open, included and not copied, so a seed is always read back
// as it was written. `records` is what the tests read the seeds back with.
#[cfg_attr(not(test), allow(dead_code))]
#[path = "../../crates/sluice-live/fuzz/framing.rs"]
mod framing;

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
    /// `sluice-core`: the `Hello` record of `sluice/1`.
    Hello,
    /// `sluice-core`: byte ranges as a person writes them.
    Range,
    /// `sluice-relay-guard`: the text of an iroh-relay config.
    #[value(name = "relay_config")]
    RelayConfig,
    /// `sluice-relay-guard`: the text of an iroh-relay config, through the guard and a loose
    /// mirror of iroh-relay's own reading. The seeds of `relay_config`, each with a choice byte
    /// in front.
    #[value(name = "relay_differential")]
    RelayDifferential,
}

impl Target {
    /// The folder of `tests/vectors/` the target is seeded from, if it has one.
    fn folder(self) -> Option<&'static str> {
        match self {
            Target::Ticket => Some("tickets"),
            Target::Manifest => Some("manifests"),
            Target::ManifestPath => Some("paths"),
            Target::Log => Some("live"),
            Target::Hello | Target::Range | Target::RelayConfig | Target::RelayDifferential => None,
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

/// Greetings: three that are valid and six that break one rule each. `Hello` has no vector folder
/// because it is three varints, and these seeds are short enough to read here. The fuzzer checks
/// that every byte string it decodes has exactly one spelling.
fn hello_seeds() -> Vec<Seed> {
    let mut bad = vec![
        // proto_version 2.
        hello(2, 0, 16 << 20),
        // A frame limit of zero.
        hello(1, 0, 0),
        // A byte after the greeting.
        {
            let mut bytes = hello(1, 0, 16 << 20);
            bytes.push(0);
            bytes
        },
        // The version as a three-byte varint, which is not its shortest spelling.
        vec![0x81, 0x80, 0x00, 0x00, 0x80, 0x80, 0x10],
        // A ten-byte varint whose tenth byte carries more than the last bit of a u64.
        vec![
            0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x02, 0x00, 0x80, 0x80, 0x10,
        ],
        // The greeting ends before its third varint.
        {
            let bytes = hello(1, 0, 16 << 20);
            bytes[..bytes.len() - 1].to_vec()
        },
    ];
    let mut seeds: Vec<Seed> = [
        hello(1, 0, 16 << 20),
        hello(1, (1 << 2) | (1 << 3), 16 << 20),
        hello(1, u64::MAX, 1),
    ]
    .into_iter()
    .map(|bytes| Seed { file: None, bytes })
    .collect();
    seeds.extend(bad.drain(..).map(|bytes| Seed { file: None, bytes }));
    seeds
}

/// The three fields of a greeting, each as its shortest varint.
fn hello(version: u64, capabilities: u64, limit: u64) -> Vec<u8> {
    let mut out = Vec::new();
    for value in [version, capabilities, limit] {
        push_varint(&mut out, value);
    }
    out
}

fn push_varint(out: &mut Vec<u8>, mut value: u64) {
    loop {
        let low = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(low);
            break;
        }
        out.push(low | 0x80);
    }
}
pub fn run(target: Target, dir: &Path) -> anyhow::Result<()> {
    let seeds = seeds(target, &vectors_dir())?;
    fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    let mut written = 0;
    for seed in &seeds {
        // Named by content, so a second run, or another target's seed with the same bytes, adds
        // no file.
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

/// Every distinct seed of `target`, from the vector files under `vectors`, each once.
pub fn seeds(target: Target, vectors: &Path) -> anyhow::Result<Vec<Seed>> {
    if matches!(target, Target::RelayConfig | Target::RelayDifferential) {
        // The vectors are in tests/vectors, two folders below the repository root.
        let configs = relay_configs(&vectors.join("..").join(".."))?;
        return Ok(match target {
            Target::RelayDifferential => with_selector(configs),
            _ => configs,
        });
    }
    if target == Target::Hello {
        return Ok(hello_seeds());
    }
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
    let mut seen = std::collections::HashSet::new();
    for (name, cases) in vector_files(&vectors.join(folder))? {
        for case in &cases {
            for bytes in from_case(target, case).with_context(|| format!("a case of {name}"))? {
                // Once each, from the first file that gives it: two cases with the same input are
                // one seed, and the count `run` prints is of the files it writes.
                if seen.insert(bytes.clone()) {
                    seeds.push(Seed {
                        file: Some(name.clone()),
                        bytes,
                    });
                }
            }
        }
    }
    Ok(seeds)
}

/// The differential target reads its first byte as the choice of whether `IROH_RELAY_ACCESS_TOKEN`
/// is set, and the rest as the config. A seed that starts with the config's own first byte, which
/// is `#` in every shipped config, leaves a comment cut in two for the rest, so the config never
/// parses and no assertion runs. So each seed is given twice, with `0x00` and with `0x01` in front.
fn with_selector(configs: Vec<Seed>) -> Vec<Seed> {
    configs
        .into_iter()
        .flat_map(|seed| {
            [0u8, 1].map(|choice| Seed {
                file: seed.file.clone(),
                bytes: [&[choice][..], &seed.bytes].concat(),
            })
        })
        .collect()
}

/// The three configs the images ship, iroh-relay 1.4.1's own examples that the guard's tests
/// parse, and [`NEAR_MISSES`]. No vector holds a relay config: it is iroh-relay's format, and not
/// the wire's.
fn relay_configs(root: &Path) -> anyhow::Result<Vec<Seed>> {
    let docker = root.join("docker");
    let examples = docker
        .join("guard")
        .join("testdata")
        .join("iroh-relay-1.4.1");
    let mut paths = vec![
        docker.join("relay").join("relay.toml"),
        docker.join("relay").join("open.toml"),
        docker.join("rendezvous").join("manual.toml"),
    ];
    let mut found = Vec::new();
    for entry in
        fs::read_dir(&examples).with_context(|| format!("cannot list {}", examples.display()))?
    {
        let path = entry
            .with_context(|| format!("cannot list {}", examples.display()))?
            .path();
        if path.extension().is_some_and(|ext| ext == "toml") {
            found.push(path);
        }
    }
    found.sort();
    paths.extend(found);
    let mut seeds = paths
        .into_iter()
        .map(|path| {
            let bytes =
                fs::read(&path).with_context(|| format!("cannot read {}", path.display()))?;
            let file = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .display()
                .to_string();
            Ok(Seed {
                file: Some(file.replace('\\', "/")),
                bytes,
            })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    seeds.extend(NEAR_MISSES.iter().map(|text| Seed {
        file: None,
        bytes: text.as_bytes().to_vec(),
    }));
    Ok(seeds)
}

/// Configs one line away from a rule of the guard, which the shipped configs and iroh-relay's
/// examples are not: none of them turns HTTPS off, leaves a limit out of an open config, or puts a
/// secret in a URL. Without a seed on each side of a rule, a run of seconds cannot tell a guard
/// that has the rule from one that lost it.
const NEAR_MISSES: [&str; 6] = [
    // `dangerous_http_only`, for a relay and for a rendezvous server.
    "enable_metrics = false\n[access]\nallowlist = []\n[tls]\ncert_mode = \"Manual\"\n\
     dangerous_http_only = true\n",
    "enable_relay = false\nenable_metrics = false\n[tls]\ncert_mode = \"Manual\"\n\
     dangerous_http_only = true\n",
    // Open with no per-client byte limit.
    "enable_metrics = false\naccess = \"everyone\"\n",
    // No `enable_metrics`.
    "[access]\nallowlist = []\n",
    // A URL with a user name, a query and a fragment.
    "enable_metrics = false\n[access.http]\nurl = \"https://u@a.example/?k=v#f\"\n",
    // An empty shared token list.
    "enable_metrics = false\n[access]\nshared_token = []\n",
];

/// The UTF-8 name of a vector file, or the error that says it is not. Split from [`vector_files`]
/// so the rule is checked on a name without a file system.
fn vector_name(name: std::ffi::OsString, dir: &Path) -> anyhow::Result<String> {
    name.into_string().map_err(|name| {
        anyhow::anyhow!(
            "{} holds a file whose name is not UTF-8: {name:?}",
            dir.display()
        )
    })
}

/// The `cases` of every `.json` file in `dir`, by file name, in name order. A name that is not
/// UTF-8 is an error and not a skip, because a vector file left out would quietly seed less.
pub fn vector_files(dir: &Path) -> anyhow::Result<Vec<(String, Vec<Value>)>> {
    let mut names = Vec::new();
    for entry in fs::read_dir(dir).with_context(|| format!("cannot list {}", dir.display()))? {
        let entry = entry.with_context(|| format!("cannot list {}", dir.display()))?;
        let name = vector_name(entry.file_name(), dir)?;
        if name.ends_with(".json") {
            names.push(name);
        }
    }
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
        Target::Log => log(case)?.into_iter().collect(),
        Target::Hello | Target::Range | Target::RelayConfig | Target::RelayDifferential => {
            Vec::new()
        }
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

/// The records after the case's open, in the target's framing, up to a second open, which the
/// framing has no tag for. The target opens its stream itself, with `framing::OPEN`, so only a
/// case that starts on those bytes gives a seed. Any other open has another stream, key or segment
/// size, and the target would read its records against the wrong one, so the case is skipped and
/// gives `None`. A case that does not start on an open, or a record longer than the framing takes,
/// is an error naming the case, and never a seed that says something else.
fn log(case: &Value) -> anyhow::Result<Option<Vec<u8>>> {
    let name = text(case, "name")?;
    let steps = case["steps"].as_array().context("no steps")?;
    let first = steps.first().map(|step| text(step, "record")).transpose()?;
    if first.as_deref() != Some("open") {
        bail!("case {name:?} does not start with an open, which the log target takes as given");
    }
    if hex_bytes(&text(&steps[0], "bytes")?).map_err(anyhow::Error::msg)? != framing::OPEN {
        return Ok(None);
    }
    let mut input = Vec::new();
    for step in &steps[1..] {
        let record = text(step, "record")?;
        if record == "open" {
            break;
        }
        let bytes = hex_bytes(&text(step, "bytes")?).map_err(anyhow::Error::msg)?;
        framing::push(&mut input, record == "close", &bytes).map_err(|len| {
            anyhow::anyhow!(
                "case {name:?} has a {record} of {len} bytes, and the log target's records are \
                 at most {} bytes",
                framing::MAX_RECORD_BYTES
            )
        })?;
    }
    Ok(Some(input))
}

#[cfg(test)]
mod tests;
