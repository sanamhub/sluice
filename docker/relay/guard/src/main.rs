//! `sluice-relay-guard [--open] --config-path <file>`: checks the config, then runs iroh-relay with
//! it. See the library for the rules, and docker/relay/README.md for what an operator sees.

use std::ffi::OsString;
use std::process::{Command, ExitCode};

use sluice_relay_guard::{Args, EXIT_REFUSED, IROH_RELAY, Refusal, check_config, read_config};

/// `EX_UNAVAILABLE` of sysexits.h: the checks passed, but iroh-relay could not be started.
const EXIT_NO_RELAY: u8 = 69;

fn main() -> ExitCode {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    match checked(&args) {
        Ok(args) => start(&args),
        Err(refusal) => {
            eprintln!("sluice-relay-guard: {refusal}");
            ExitCode::from(EXIT_REFUSED)
        }
    }
}

fn checked(args: &[OsString]) -> Result<Args, Refusal> {
    let args = Args::parse(args)?;
    let text = read_config(&args.config_path)?;
    check_config(&text, args.open)?;
    Ok(args)
}

/// The relay as it would run without the guard, with the file the guard read. `--config-path=` in
/// one argument, so a path that starts with `-` cannot be read as a flag.
fn relay(args: &Args) -> Command {
    let mut flag = OsString::from("--config-path=");
    flag.push(&args.config_path);
    let mut command = Command::new(IROH_RELAY);
    command.arg(flag);
    command
}

/// Replaces this process, so iroh-relay is PID 1 in the container and gets its signals directly.
#[cfg(unix)]
fn start(args: &Args) -> ExitCode {
    use std::os::unix::process::CommandExt;

    let err = relay(args).exec();
    eprintln!("sluice-relay-guard: cannot start {IROH_RELAY}: {err}");
    ExitCode::from(EXIT_NO_RELAY)
}

/// The image is Linux only. Elsewhere the guard only has to build and test, so it waits for the
/// relay as a child and passes on its exit code.
#[cfg(not(unix))]
fn start(args: &Args) -> ExitCode {
    match relay(args).status() {
        Ok(status) => match status.code() {
            Some(code) => ExitCode::from(u8::try_from(code).unwrap_or(1)),
            None => ExitCode::FAILURE,
        },
        Err(err) => {
            eprintln!("sluice-relay-guard: cannot start {IROH_RELAY}: {err}");
            ExitCode::from(EXIT_NO_RELAY)
        }
    }
}
