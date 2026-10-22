//! The command line, `sluice-relay-guard [--open] --config-path <file>`, and nothing else.

use std::ffi::{OsStr, OsString};
use std::path::PathBuf;

use crate::Refusal;

/// What the guard was asked to start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Args {
    /// `--open`: the operator means to run an open relay, with a per-client byte limit.
    pub open: bool,
    /// `--config-path`: the config file the relay is started with.
    pub config_path: PathBuf,
}

impl Args {
    /// Reads the arguments after the program name. `--config-path` is required, and may be written
    /// `--config-path <file>` or `--config-path=<file>`. Anything else is refused, `--dev` and
    /// `--help` included: iroh-relay's `--dev` turns TLS off and runs on any config.
    ///
    /// ```
    /// use std::ffi::OsString;
    /// use sluice_relay_guard::Args;
    ///
    /// let args: Vec<OsString> = ["--open", "--config-path", "/etc/relay/open.toml"]
    ///     .iter()
    ///     .map(OsString::from)
    ///     .collect();
    /// let parsed = Args::parse(&args).unwrap();
    /// assert!(parsed.open);
    /// assert_eq!(parsed.config_path, std::path::Path::new("/etc/relay/open.toml"));
    ///
    /// assert!(Args::parse(&[OsString::from("--dev")]).is_err());
    /// ```
    pub fn parse(args: &[OsString]) -> Result<Args, Refusal> {
        let mut open = false;
        let mut config_path: Option<PathBuf> = None;
        let mut rest = args.iter();
        while let Some(arg) = rest.next() {
            let path = if arg == "--open" {
                if open {
                    return Err(Refusal::new("--open is given twice: give it once"));
                }
                open = true;
                continue;
            } else if arg == "--config-path" {
                match rest.next() {
                    Some(path) => path.clone(),
                    None => return Err(missing_path()),
                }
            } else if let Some(path) = value_of(arg) {
                path
            } else {
                return Err(unknown(arg));
            };
            if path.is_empty() {
                return Err(missing_path());
            }
            if config_path.replace(PathBuf::from(path)).is_some() {
                return Err(Refusal::new("--config-path is given twice: give it once"));
            }
        }
        match config_path {
            Some(config_path) => Ok(Args { open, config_path }),
            None => Err(Refusal::new(
                "--config-path <file> is required: without a config file iroh-relay runs open, \
                 with no limit",
            )),
        }
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

fn unknown(arg: &OsStr) -> Refusal {
    if arg == "--dev" {
        return Refusal::new(
            "--dev is refused: it turns TLS off and is meant for a relay on a laptop; remove it",
        );
    }
    Refusal::new(format!(
        "unknown argument `{}`: the guard takes only [--open] --config-path <file>",
        arg.to_string_lossy()
    ))
}
