//! `sluice-relay-guard [--open | --rendezvous] --config-path <file>`: checks the config, then
//! runs iroh-relay with it. See the library for the rules, and docker/guard/README.md for what an
//! operator sees.

use std::ffi::OsString;
use std::fs::File;
use std::process::ExitCode;

use sluice_relay_guard::{
    ACCESS_TOKEN_VAR, Args, EXIT_REFUSED, Mode, Refusal, check_config, open_config,
};

/// `EX_UNAVAILABLE` of sysexits.h: the checks passed, but iroh-relay could not be started.
const EXIT_NO_RELAY: u8 = 69;

fn main() -> ExitCode {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    match checked(&args) {
        Ok((mode, config)) => start(mode, config),
        Err(refusal) => {
            eprintln!("sluice-relay-guard: {refusal}");
            ExitCode::from(EXIT_REFUSED)
        }
    }
}

/// The mode asked for, and the config file, open and checked.
fn checked(args: &[OsString]) -> Result<(Mode, File), Refusal> {
    let args = Args::parse(args)?;
    let (file, text) = open_config(&args.config_path)?;
    // As iroh-relay reads it: a value that is not UTF-8 counts as unset there too.
    let access_token = std::env::var(ACCESS_TOKEN_VAR).ok();
    check_config(&text, args.mode, access_token.as_deref())?;
    Ok((args.mode, file))
}

/// Replaces this process with iroh-relay, whose standard input is the file the guard checked, so
/// iroh-relay is PID 1 in the container and gets its signals directly. Before it does, it proves
/// that `/proc/self/fd/<n>` is that file: iroh-relay runs open when it finds no config, so a
/// runtime without a usable `/proc` must stop here and not there.
#[cfg(unix)]
fn start(mode: Mode, config: File) -> ExitCode {
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};

    use sluice_relay_guard::{IROH_RELAY, check_handover, relay_args};

    if let Err(refusal) = check_handover(&config) {
        eprintln!("sluice-relay-guard: {refusal}");
        return ExitCode::from(EXIT_NO_RELAY);
    }
    let err = Command::new(IROH_RELAY)
        .args(relay_args(mode))
        .stdin(Stdio::from(config))
        .exec();
    eprintln!("sluice-relay-guard: cannot start {IROH_RELAY}: {err}");
    ExitCode::from(EXIT_NO_RELAY)
}

/// The image is Linux only, and the handover needs `/proc/self/fd`. Elsewhere iroh-relay would
/// find no config at that path and run its open default, so the guard checks and stops.
#[cfg(not(unix))]
fn start(_mode: Mode, _config: File) -> ExitCode {
    eprintln!(
        "sluice-relay-guard: the config passed, but iroh-relay is started only on Unix, where \
         /proc/self/fd hands it the file that was checked"
    );
    ExitCode::from(EXIT_NO_RELAY)
}
