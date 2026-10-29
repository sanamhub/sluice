//! The `sluice` command line. The binary in `main.rs` calls [`run`]. The library exists so that
//! the completion scripts and the man pages can be made from the same clap definition.

mod bug;
mod cli;
mod colour;
mod completions;
mod doctor;
mod events;
mod exit;
mod id;
mod inspect;
mod network;
mod output;
mod queue;
mod recv;
mod send;

use std::ffi::OsString;
use std::process::ExitCode;
use std::time::Duration;

use clap::{CommandFactory, Parser};

use cli::{Cli, Command};
use colour::Environment;
use exit::Failure;
use output::Output;

/// How long a command waits for the removals it started before it ends. A removal waits for a file
/// another program may hold, and the waits add up to a little over a second; two seconds covers one
/// of those and a retry, and anything still left is removed by the sweep of temporary folders.
const REMOVAL_WAIT: Duration = Duration::from_secs(2);

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
    let code = run_to_exit(args);
    // On every way out, including a bad command line and a panic: a removal that is still running
    // has a thread of its own, which neither the end of the runtime nor the end of the process
    // waits for, and the folder it is removing would stay until the sweep.
    sluice_core::engine::wait_for_removals(REMOVAL_WAIT);
    code
}

/// Parses the arguments, runs the command and prints what it produced.
fn run_to_exit(args: Vec<OsString>) -> ExitCode {
    bug::install_hook();
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
    // A panic is caught here, so that it ends as a `Bug` and not as Rust's 101.
    match bug::catch(|| execute(cli.command, &output)) {
        Ok(Ok(code)) => ExitCode::from(code),
        Ok(Err(error)) => output.print_failure(&Failure::Error(error)),
        Err(message) => output.print_failure(&Failure::Panic(message)),
    }
}

/// Runs the command and prints what it produced, and returns the exit code that goes with it.
fn execute(command: Command, output: &Output) -> Result<u8, sluice_core::Error> {
    #[cfg(debug_assertions)]
    bug::panic_if_asked();
    // A command that was killed cannot remove the temporary folder it made. Its own `tmp` folder is
    // private to this user, so sweeping it by age is safe, and it is never done to the output
    // folder. Only the commands that make such folders sweep it: `doctor` promises to create
    // nothing, and `id` and `completions` never make one. Best effort, and tried again next time.
    if matches!(
        &command,
        Command::Send(_) | Command::Recv(_) | Command::Inspect(_)
    ) {
        let _ = sluice_core::engine::sweep_temporary_store(std::time::SystemTime::now());
    }
    let outcome = match command {
        Command::Id(args) => id::run(&args, output.format())?,
        Command::Completions(args) => completions::run(&args)?,
        Command::Send(args) => return send::run(&args, output),
        Command::Recv(args) => return recv::run(&args, output),
        Command::Inspect(args) => inspect::run(&args)?,
        Command::Doctor(args) => doctor::run(&args)?,
    };
    output.print(&outcome)?;
    Ok(outcome.code())
}
