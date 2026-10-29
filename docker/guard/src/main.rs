//! `sluice-relay-guard [--open | --behind-proxy] --config-path <file>`: checks the config, then
//! runs iroh-relay with a sealed copy of it. See the library for the rules, and
//! docker/guard/README.md for what an operator sees.

use std::ffi::OsString;
use std::fs::File;
use std::process::ExitCode;

use sluice_relay_guard::{
    ACCESS_TOKEN_VAR, Args, EXIT_REFUSED, Refusal, check_config, hand_over, open_config,
};

/// `EX_UNAVAILABLE` of sysexits.h: the checks passed, but iroh-relay could not be started.
const EXIT_NO_RELAY: u8 = 69;

fn main() -> ExitCode {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    match checked(&args) {
        Ok((config, text)) => start(config, text),
        Err(refusal) => {
            eprintln!("sluice-relay-guard: {refusal}");
            ExitCode::from(EXIT_REFUSED)
        }
    }
}

/// The config file, open and checked, with the bytes that were checked.
fn checked(args: &[OsString]) -> Result<(File, String), Refusal> {
    let args = Args::parse(args)?;
    let (file, text) = open_config(&args.config_path)?;
    // As iroh-relay reads it: a value that is not UTF-8 counts as unset there too.
    let access_token = std::env::var(ACCESS_TOKEN_VAR).ok();
    check_config(&text, args.mode, access_token.as_deref(), args.behind_proxy)?;
    Ok((file, text))
}

/// Replaces this process with iroh-relay, whose descriptor 0 holds the sealed copy of the bytes the
/// guard checked (ADR 0021), so iroh-relay is PID 1 in the container and gets its signals
/// directly. Before it does, the handover proves that `/proc/self/fd/0` is that copy: iroh-relay
/// runs open when it finds no config, so a runtime without a usable `/proc` must stop here and not
/// there.
#[cfg(target_os = "linux")]
fn start(config: File, text: String) -> ExitCode {
    use std::os::unix::process::CommandExt;
    use std::process::Command;

    use sluice_relay_guard::{IROH_RELAY, relay_args};

    if let Err(refusal) = hand_over(config, &text) {
        eprintln!("sluice-relay-guard: {refusal}");
        return ExitCode::from(EXIT_NO_RELAY);
    }
    // Standard input is inherited, and is the sealed copy.
    let err = Command::new(IROH_RELAY).args(relay_args()).exec();
    eprintln!("sluice-relay-guard: cannot start {IROH_RELAY}: {err}");
    ExitCode::from(EXIT_NO_RELAY)
}

/// The image is Linux only, and the sealed copy needs `memfd_create` and `/proc/self/fd`. Elsewhere
/// the handover refuses, and the guard prints why and stops.
#[cfg(not(target_os = "linux"))]
fn start(config: File, text: String) -> ExitCode {
    if let Err(refusal) = hand_over(config, &text) {
        eprintln!("sluice-relay-guard: {refusal}");
    }
    ExitCode::from(EXIT_NO_RELAY)
}
