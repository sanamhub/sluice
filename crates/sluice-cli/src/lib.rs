//! The `sluice` command line. The binary in `main.rs` calls [`run`]. The library exists so that
//! the completion scripts and the man pages can be made from the same clap definition.

mod cli;
mod colour;
mod exit;
mod id;
mod output;

use std::ffi::OsString;
use std::process::ExitCode;

use clap::{CommandFactory, Parser};

use cli::{Cli, Command};
use colour::Environment;
use output::Output;

/// The clap definition of the whole command line, for the completion scripts and the man pages.
///
/// # Examples
///
/// ```
/// let command = sluice_cli::command();
/// assert_eq!(command.get_name(), "sluice");
/// assert!(command.get_subcommands().any(|sub| sub.get_name() == "id"));
/// ```
pub fn command() -> clap::Command {
    Cli::command()
}

/// Runs `sluice` with these arguments, the first being the program name, and returns the exit
/// code. Results go to stdout and failures to stderr.
///
/// # Examples
///
/// ```
/// use std::ffi::OsString;
///
/// let args: Vec<OsString> = vec!["sluice".into(), "--version".into()];
/// assert_eq!(sluice_cli::run(args), std::process::ExitCode::SUCCESS);
/// ```
pub fn run(args: Vec<OsString>) -> ExitCode {
    let environment = Environment::read();
    let cli = match Cli::try_parse_from(&args) {
        Ok(cli) => cli,
        // The flags are not parsed, so `--color` is read out of the raw arguments.
        Err(error) => {
            let output = Output::new(
                output::Format::Human,
                environment.colour(colour::peek(&args)),
            );
            return output.print_clap(&error);
        }
    };
    let output = Output::new(cli.format(), environment.colour(cli.color));
    match execute(cli.command, &output) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => output.print_failure(&error),
    }
}

fn execute(command: Command, output: &Output) -> Result<(), sluice_core::Error> {
    let outcome = match command {
        Command::Id(args) => id::run(&args, output.format())?,
    };
    output.print(&outcome)
}
