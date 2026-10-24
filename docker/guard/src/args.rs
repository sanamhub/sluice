//! The command line, `sluice-relay-guard [--open | --rendezvous | --behind-proxy]
//! --config-path <file>`, and nothing else. `--open` and `--behind-proxy` may be given together:
//! a relay that runs open can still stand behind a proxy that ends TLS.

use std::ffi::{OsStr, OsString};
use std::path::PathBuf;

use crate::{Mode, Refusal};

/// What the guard was asked to start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Args {
    /// [`Mode::Open`] for `--open`, [`Mode::Rendezvous`] for `--rendezvous`, and
    /// [`Mode::Closed`] for neither.
    pub mode: Mode,
    /// `--behind-proxy`: a relay whose config has no `[tls]`, in front of a proxy that ends TLS.
    pub behind_proxy: bool,
    /// `--config-path`: the config file the relay is started with.
    pub config_path: PathBuf,
}

impl Args {
    /// Reads the arguments after the program name. `--config-path` is required, and may be written
    /// `--config-path <file>` or `--config-path=<file>`. `--open`, `--rendezvous` and
    /// `--behind-proxy` are each allowed once; `--open` and `--rendezvous` are not given together,
    /// and `--behind-proxy` is refused with `--rendezvous`, where it has nothing to stand behind.
    /// Anything else is refused, `--dev` and `--help` included: iroh-relay's `--dev` turns TLS off
    /// and runs on any config.
    ///
    /// ```
    /// use std::ffi::OsString;
    /// use sluice_relay_guard::{Args, Mode};
    ///
    /// let args: Vec<OsString> = ["--open", "--config-path", "/etc/relay/open.toml"]
    ///     .iter()
    ///     .map(OsString::from)
    ///     .collect();
    /// let parsed = Args::parse(&args).unwrap();
    /// assert_eq!(parsed.mode, Mode::Open);
    /// assert!(!parsed.behind_proxy);
    /// assert_eq!(parsed.config_path, std::path::Path::new("/etc/relay/open.toml"));
    ///
    /// let behind: Vec<OsString> = ["--behind-proxy", "--config-path", "/etc/relay/a.toml"]
    ///     .iter()
    ///     .map(OsString::from)
    ///     .collect();
    /// assert!(Args::parse(&behind).unwrap().behind_proxy);
    /// assert!(Args::parse(&[OsString::from("--dev")]).is_err());
    /// ```
    pub fn parse(args: &[OsString]) -> Result<Args, Refusal> {
        let mut mode: Option<Mode> = None;
        let mut behind_proxy = false;
        let mut config_path: Option<PathBuf> = None;
        let mut rest = args.iter();
        let mut position = 0;
        while let Some(arg) = rest.next() {
            position += 1;
            let path = if let Some(flag) = mode_flag(arg) {
                mode = Some(both(mode, flag)?);
                continue;
            } else if arg == "--behind-proxy" {
                if behind_proxy {
                    return Err(Refusal::new("--behind-proxy is given twice: give it once"));
                }
                behind_proxy = true;
                continue;
            } else if arg == "--config-path" {
                position += 1;
                match rest.next() {
                    Some(path) => path.clone(),
                    None => return Err(missing_path()),
                }
            } else if let Some(path) = value_of(arg) {
                path
            } else {
                return Err(unknown(arg, position));
            };
            if path.is_empty() {
                return Err(missing_path());
            }
            if config_path.replace(PathBuf::from(path)).is_some() {
                return Err(Refusal::new("--config-path is given twice: give it once"));
            }
        }
        let mode = mode.unwrap_or(Mode::Closed);
        if behind_proxy && mode == Mode::Rendezvous {
            return Err(Refusal::new(
                "--behind-proxy is for a relay behind a proxy that ends TLS, and a rendezvous \
                 server relays nothing: remove it, or start a relay without --rendezvous",
            ));
        }
        match config_path {
            Some(config_path) => Ok(Args {
                mode,
                behind_proxy,
                config_path,
            }),
            None => Err(Refusal::new(
                "--config-path <file> is required: without a config file iroh-relay runs open, \
                 with no limit",
            )),
        }
    }
}

fn mode_flag(arg: &OsStr) -> Option<Mode> {
    match arg.to_str()? {
        "--open" => Some(Mode::Open),
        "--rendezvous" => Some(Mode::Rendezvous),
        _ => None,
    }
}

/// The mode after a second flag, which may neither repeat the first nor contradict it.
fn both(before: Option<Mode>, flag: Mode) -> Result<Mode, Refusal> {
    match before {
        None => Ok(flag),
        Some(before) if before == flag => Err(Refusal::new(format!(
            "{} is given twice: give it once",
            flag_name(flag)
        ))),
        Some(_) => Err(Refusal::new(
            "--open and --rendezvous are given together: a rendezvous server relays nothing, so \
             there is nothing to open; give one of them",
        )),
    }
}

fn flag_name(mode: Mode) -> &'static str {
    match mode {
        Mode::Open => "--open",
        Mode::Rendezvous => "--rendezvous",
        Mode::Closed => "no flag",
    }
}

/// The file of `--config-path=<file>`, if `arg` is written that way. A path that is not UTF-8 has
/// to be given as `--config-path <file>`.
fn value_of(arg: &OsStr) -> Option<OsString> {
    arg.to_str()?
        .strip_prefix("--config-path=")
        .map(OsString::from)
}

fn missing_path() -> Refusal {
    Refusal::new("--config-path needs the path of the config file after it")
}

/// An unknown argument is text from the command line, and may be a token pasted in the wrong
/// place, or hold a terminal escape code. It is printed only when it looks like a flag: `--`, then
/// 1 to 32 lower-case letters or `-`, as every flag iroh-relay has is. Any other is "an argument",
/// with its 1-based position, so that a token on the command line stays out of the log.
fn unknown(arg: &OsStr, position: usize) -> Refusal {
    if arg == "--dev" {
        return Refusal::new(
            "--dev is refused: it turns TLS off and is meant for a relay on a laptop; remove it",
        );
    }
    let named = match arg.to_str().filter(|arg| looks_like_a_flag(arg)) {
        Some(flag) => format!("unknown argument `{flag}`"),
        None => format!("an argument at position {position} is unknown"),
    };
    Refusal::new(format!(
        "{named}: the guard takes only [--open | --rendezvous | --behind-proxy] \
         --config-path <file>"
    ))
}

/// `--` and then 1 to 32 characters of `a-z` or `-`.
fn looks_like_a_flag(arg: &str) -> bool {
    arg.strip_prefix("--").is_some_and(|name| {
        (1..=32).contains(&name.len()) && name.bytes().all(|b| b.is_ascii_lowercase() || b == b'-')
    })
}
