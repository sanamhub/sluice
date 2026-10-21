//! The `sluice` binary.

use std::process::ExitCode;

fn main() -> ExitCode {
    sluice_cli::run(std::env::args_os().collect())
}
