//! The `sluice` binary.

mod id;

use std::process::ExitCode;

use clap::{Parser, Subcommand};
use sluice_core::Error;

#[derive(Parser)]
#[command(name = "sluice", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Print the endpoint id of this machine's identity, or replace the identity
    Id(id::Args),
}

fn main() -> ExitCode {
    // clap reports a bad command line itself, and exits with 2.
    let cli = Cli::parse();
    match run(cli.command) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("sluice: {error}");
            ExitCode::from(exit_code(&error))
        }
    }
}

fn run(command: Command) -> Result<(), Error> {
    match command {
        Command::Id(args) => id::run(&args),
    }
}

/// The exit code for an error, from the variant alone, as the table under Errors in
/// `docs/design.md` gives them. Every command goes through this one function, so a script can rely
/// on a code meaning the same thing whatever it ran.
fn exit_code(error: &Error) -> u8 {
    match error {
        Error::Network(_) => 3,
        Error::Verify(_) => 4,
        Error::Storage(_) => 5,
        Error::Policy(_) => 6,
        Error::Cancelled => 130,
        Error::Bug(_) => 70,
    }
}

#[cfg(test)]
mod tests {
    use super::{Error, exit_code};

    #[test]
    fn each_variant_has_the_code_the_design_gives_it() {
        let cases = [
            (Error::Network(String::new()), 3),
            (Error::Verify(String::new()), 4),
            (Error::Storage(String::new()), 5),
            (Error::Policy(String::new()), 6),
            (Error::Cancelled, 130),
            (Error::Bug(String::new()), 70),
        ];
        for (error, code) in cases {
            assert_eq!(exit_code(&error), code, "{error}");
        }
    }
}
