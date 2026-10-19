//! The `sluice` binary.

use clap::Parser;

#[derive(Parser)]
#[command(name = "sluice", version)]
struct Cli {}

fn main() {
    Cli::parse();
}
