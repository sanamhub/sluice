//! `sluice-relay-guard [--open] --config-path <file>`: checks the config, then runs iroh-relay with
//! it. See the library for the rules, and docker/relay/README.md for what an operator sees.

use std::ffi::OsString;
use std::fs::File;
use std::process::ExitCode;

use sluice_relay_guard::{Args, EXIT_REFUSED, Refusal, check_config, open_config};

/// `EX_UNAVAILABLE` of sysexits.h: the checks passed, but iroh-relay could not be started.
const EXIT_NO_RELAY: u8 = 69;

fn main() -> ExitCode {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    match checked(&args) {
        Ok(config) => start(config),
        Err(refusal) => {
            eprintln!("sluice-relay-guard: {refusal}");
            ExitCode::from(EXIT_REFUSED)
        }
    }
}

/// The config file, open and checked.
fn checked(args: &[OsString]) -> Result<File, Refusal> {
    let args = Args::parse(args)?;
    let (file, text) = open_config(&args.config_path)?;
    check_config(&text, args.open)?;
    Ok(file)
}

/// Replaces this process with iroh-relay, whose standard input is the file the guard checked, so
/// iroh-relay is PID 1 in the container and gets its signals directly.
#[cfg(unix)]
fn start(config: File) -> ExitCode {
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};

    use sluice_relay_guard::{IROH_RELAY, relay_args};

    let err = Command::new(IROH_RELAY)
        .args(relay_args())
        .stdin(Stdio::from(config))
        .exec();
    eprintln!("sluice-relay-guard: cannot start {IROH_RELAY}: {err}");
    ExitCode::from(EXIT_NO_RELAY)
}

/// The image is Linux only, and the handover needs `/dev/stdin`. Without it iroh-relay would find
/// no config at that path and run its open default, so elsewhere the guard checks and stops.
#[cfg(not(unix))]
fn start(_config: File) -> ExitCode {
    eprintln!(
        "sluice-relay-guard: the config passed, but iroh-relay is started only on Unix, where \
         /dev/stdin hands it the file that was checked"
    );
    ExitCode::from(EXIT_NO_RELAY)
}
