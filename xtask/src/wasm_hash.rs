//! `cargo xtask wasm-hash`: builds `sluice-wasm-hash` twice for the browser, runs `wasm-bindgen`
//! on both builds, and writes what `bench/wasm-hash/page` loads.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs};

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};

const CRATE: &str = "sluice-wasm-hash";
const TARGET: &str = "wasm32-unknown-unknown";
const OUT_NAME: &str = "sluice_wasm_hash";
const BUDGET_BROTLI_BYTES: u64 = 1_500_000;

struct Variant {
    name: &'static str,
    features: &'static [&'static str],
    rustflags: &'static str,
}

// The two builds differ only here. `simd` is the build ADR 0014 names. `scalar` shows what SIMD is
// worth, and what a browser without it would get.
const VARIANTS: [Variant; 2] = [
    Variant {
        name: "simd",
        features: &["simd"],
        rustflags: "-Ctarget-feature=+simd128",
    },
    Variant {
        name: "scalar",
        features: &[],
        rustflags: "",
    },
];

#[derive(Deserialize)]
struct Metadata {
    target_directory: PathBuf,
    packages: Vec<Package>,
    resolve: Resolve,
}

#[derive(Deserialize)]
struct Package {
    id: String,
    name: String,
    version: String,
}

#[derive(Deserialize)]
struct Resolve {
    nodes: Vec<Node>,
}

#[derive(Deserialize)]
struct Node {
    id: String,
    deps: Vec<Dep>,
}

#[derive(Deserialize)]
struct Dep {
    name: String,
    pkg: String,
}

#[derive(Serialize)]
struct Sizes {
    raw_bytes: u64,
    brotli_bytes: u64,
}

pub fn run() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .context("xtask is not inside the workspace")?
        .to_path_buf();
    let cargo = env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));
    let meta = metadata(&cargo, &root)?;
    check_cli_version(&crate_bindgen_version(&meta)?)?;

    let generated = root.join("bench/wasm-hash/page/src/generated");
    let mut sizes = BTreeMap::new();
    for variant in &VARIANTS {
        let wasm = build(&cargo, &root, &meta.target_directory, variant)?;
        let out_dir = generated.join(variant.name);
        bindgen(&wasm, &out_dir)?;
        sizes.insert(
            variant.name,
            measure(&out_dir.join(format!("{OUT_NAME}_bg.wasm")))?,
        );
    }

    let mut json = serde_json::to_string_pretty(&sizes)?;
    json.push('\n');
    fs::write(generated.join("sizes.json"), json)?;
    print_sizes(&sizes);
    Ok(())
}

fn metadata(cargo: &OsString, root: &Path) -> Result<Metadata> {
    let out = Command::new(cargo)
        .current_dir(root)
        .args([
            "metadata",
            "--format-version",
            "1",
            "--filter-platform",
            TARGET,
        ])
        .output()
        .context("cannot run cargo metadata")?;
    ensure!(
        out.status.success(),
        "cargo metadata failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).context("cannot read cargo metadata")
}

/// The version of `wasm-bindgen` that `sluice-wasm-hash` resolves to. Other crates in the lockfile
/// can pull other versions, so the crate's own dependency edge is the one that counts.
fn crate_bindgen_version(meta: &Metadata) -> Result<String> {
    let krate = meta
        .packages
        .iter()
        .find(|p| p.name == CRATE)
        .with_context(|| format!("{CRATE} is not in the workspace"))?;
    let node = meta
        .resolve
        .nodes
        .iter()
        .find(|n| n.id == krate.id)
        .with_context(|| format!("{CRATE} is not in the resolved graph"))?;
    let dep = node
        .deps
        .iter()
        .find(|d| d.name == "wasm_bindgen")
        .with_context(|| format!("{CRATE} does not depend on wasm-bindgen"))?;
    let package = meta
        .packages
        .iter()
        .find(|p| p.id == dep.pkg)
        .context("wasm-bindgen is missing from the package list")?;
    Ok(package.version.clone())
}

/// The CLI rewrites the module against a private ABI that changes with every release, and a
/// mismatch fails at load time in the browser with an error that does not name the cause.
fn check_cli_version(crate_version: &str) -> Result<()> {
    let install = format!("cargo install wasm-bindgen-cli --version {crate_version} --locked");
    let out = Command::new("wasm-bindgen")
        .arg("--version")
        .output()
        .with_context(|| format!("wasm-bindgen is not installed. Run: {install}"))?;
    let text = String::from_utf8_lossy(&out.stdout);
    let cli_version = text.split_whitespace().nth(1).unwrap_or_default();
    ensure!(
        cli_version == crate_version,
        "wasm-bindgen CLI is {cli_version} but {CRATE} uses wasm-bindgen {crate_version}. Run: {install}"
    );
    Ok(())
}

fn build(cargo: &OsString, root: &Path, target_dir: &Path, variant: &Variant) -> Result<PathBuf> {
    // Separate target directories keep each variant's dependencies cached. The flags change between
    // them, and one shared directory would rebuild everything on every run.
    let dir = target_dir.join("wasm-hash").join(variant.name);
    let mut cmd = Command::new(cargo);
    cmd.current_dir(root)
        .args(["build", "--release", "--target", TARGET, "--package", CRATE])
        .arg("--target-dir")
        .arg(&dir);
    // Set here and not in the root Cargo.toml: no other crate in the workspace wants fat LTO.
    for setting in [
        "profile.release.lto=\"fat\"",
        "profile.release.codegen-units=1",
        "profile.release.panic=\"abort\"",
    ] {
        cmd.args(["--config", setting]);
    }
    if !variant.features.is_empty() {
        cmd.args(["--features", &variant.features.join(",")]);
    }
    // A caller's RUSTFLAGS would put +simd128 into the scalar build or drop it from the simd one.
    cmd.env_remove("RUSTFLAGS");
    if variant.rustflags.is_empty() {
        cmd.env_remove("CARGO_ENCODED_RUSTFLAGS");
    } else {
        cmd.env("CARGO_ENCODED_RUSTFLAGS", variant.rustflags);
    }
    println!("building {} ({TARGET}, release)", variant.name);
    let status = cmd.status().context("cannot run cargo build")?;
    if !status.success() {
        bail!(
            "the {} build failed. If the target is missing, run: rustup target add {TARGET}",
            variant.name
        );
    }
    Ok(dir
        .join(TARGET)
        .join("release")
        .join(format!("{OUT_NAME}.wasm")))
}

fn bindgen(wasm: &Path, out_dir: &Path) -> Result<()> {
    // A file the CLI no longer writes would otherwise stay behind and still be bundled.
    if out_dir.exists() {
        fs::remove_dir_all(out_dir)?;
    }
    fs::create_dir_all(out_dir)?;
    let status = Command::new("wasm-bindgen")
        .arg(wasm)
        .args(["--target", "web", "--out-name", OUT_NAME])
        .args(["--remove-name-section", "--remove-producers-section"])
        .arg("--out-dir")
        .arg(out_dir)
        .status()
        .context("cannot run wasm-bindgen")?;
    ensure!(status.success(), "wasm-bindgen failed");
    Ok(())
}

fn measure(wasm: &Path) -> Result<Sizes> {
    let raw = fs::read(wasm).with_context(|| format!("cannot read {}", wasm.display()))?;
    let mut packed = Vec::new();
    {
        // Quality 11 and a 4 MiB window are what a static host's pre-compressed `.br` file uses.
        let mut encoder = brotli::CompressorWriter::new(&mut packed, 4096, 11, 22);
        encoder.write_all(&raw)?;
    }
    Ok(Sizes {
        raw_bytes: raw.len() as u64,
        brotli_bytes: packed.len() as u64,
    })
}

fn print_sizes(sizes: &BTreeMap<&str, Sizes>) {
    println!();
    println!(
        "{:<8} {:>12} {:>14}",
        "variant", "raw bytes", "brotli bytes"
    );
    for (name, s) in sizes {
        println!("{name:<8} {:>12} {:>14}", s.raw_bytes, s.brotli_bytes);
    }
    println!(
        "budget (docs/performance.md): {BUDGET_BROTLI_BYTES} brotli bytes for the whole WASM build"
    );
}
