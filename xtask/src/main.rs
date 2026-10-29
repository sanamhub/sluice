//! The workspace's task runner. Run it as `cargo xtask <command>`, through the alias in
//! `.cargo/config.toml`.

mod cold_start;
mod conformance;
mod fuzz_seeds;
mod man;
mod package;
mod tree;
mod unicode_ignorable;
mod wasm_features;
mod wasm_hash;

use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "xtask", about = "Workspace tasks")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Build both `sluice-wasm-hash` variants and write the assets the browser page loads
    WasmHash,
    /// Run every test vector under `tests/vectors/` against this implementation
    Conformance,
    /// Build `sluice` in release and fail if a median start takes over 30 ms
    ColdStart,
    /// Write the seed inputs of one fuzz target into a folder, from the test vectors
    FuzzSeeds {
        /// The fuzz target, by its name in its crate's `fuzz/Cargo.toml`
        #[arg(value_enum)]
        target: fuzz_seeds::Target,
        /// The corpus folder to write the seeds into, which is created if it is missing
        dir: PathBuf,
    },
    /// Write the man pages of `sluice` into a folder
    Man {
        /// The folder to write `sluice.1` and one page per subcommand into
        dir: PathBuf,
    },
    /// Write the Homebrew, Scoop, winget and AUR files of a release from its checksums
    Package {
        /// The release version, without its leading `v`
        version: String,
        /// The folder that holds the release's `.sha256` files
        #[arg(long, value_name = "DIR")]
        from: PathBuf,
        /// The folder to write the files into
        #[arg(long, value_name = "DIR", default_value = "packaging")]
        out: PathBuf,
    },
    /// Fail if the tree holds a file a build writes or a top-level entry that does not belong
    Tree,
    /// Write one Unicode property's ranges from the character database as Rust source
    UnicodeIgnorable {
        /// The `DerivedCoreProperties.txt` file of the Unicode version the tables are pinned to
        source: PathBuf,
        /// The Rust file to write the ranges into
        out: PathBuf,
    },
}

fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        Command::WasmHash => wasm_hash::run(),
        Command::Conformance => conformance::run(),
        Command::ColdStart => cold_start::run(),
        Command::FuzzSeeds { target, dir } => fuzz_seeds::run(target, &dir),
        Command::Man { dir } => man::run(&dir),
        Command::Package { version, from, out } => package::run(&version, &from, &out),
        Command::Tree => tree::run(),
        Command::UnicodeIgnorable { source, out } => unicode_ignorable::run(&source, &out),
    }
}
