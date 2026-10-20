//! The workspace's task runner. Run it as `cargo xtask <command>`, through the alias in
//! `.cargo/config.toml`.

mod wasm_features;
mod wasm_hash;

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
}

fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        Command::WasmHash => wasm_hash::run(),
    }
}
