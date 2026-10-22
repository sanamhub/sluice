//! The config file, opened once, and the way iroh-relay is pointed at it.
//!
//! iroh-relay opens its config path itself (main.rs:542-567). If the guard passed it the path,
//! a rename between the guard's read and the relay's, such as a Kubernetes ConfigMap update, would
//! start a file nobody checked. So the guard keeps the file it read open, and hands that same file
//! to iroh-relay as its standard input, with `--config-path=/dev/stdin` (ADR 0020).

use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

use crate::Refusal;

/// The path iroh-relay is told to read: its own standard input, which is the file the guard
/// checked. Linux opens `/dev/stdin` as that file again, from its start, so a rename of the path
/// after the check cannot change what the relay reads.
///
/// ```
/// assert_eq!(sluice_relay_guard::CONFIG_ON_STDIN, "/dev/stdin");
/// ```
pub const CONFIG_ON_STDIN: &str = "/dev/stdin";

/// Opens the config file and reads it. The file must exist and be a regular file: iroh-relay runs
/// its open default when the path does not exist. It returns the open file, at its start, with
/// the text read from it, so the relay can be handed the bytes that were checked.
///
/// ```
/// use std::io::Read;
/// use sluice_relay_guard::open_config;
///
/// assert!(open_config(std::path::Path::new("/no/such/relay.toml")).is_err());
/// assert!(open_config(&std::env::temp_dir()).is_err());
///
/// let path = std::env::temp_dir().join(format!("guard-doc-{}.toml", std::process::id()));
/// std::fs::write(&path, "enable_metrics = false\n").unwrap();
/// let (mut file, text) = open_config(&path).unwrap();
/// assert_eq!(text, "enable_metrics = false\n");
/// let mut again = String::new();
/// file.read_to_string(&mut again).unwrap();
/// assert_eq!(again, text);
/// drop(file);
/// std::fs::remove_file(path).unwrap();
/// ```
pub fn open_config(path: &Path) -> Result<(File, String), Refusal> {
    let shown = path.display();
    // The path first, so that a FIFO or a device is refused before an open that could block.
    let named = fs::metadata(path).map_err(|err| cannot_open(path, &err))?;
    if !named.is_file() {
        return Err(not_a_file(path));
    }
    let mut file = File::open(path).map_err(|err| cannot_open(path, &err))?;
    // The open file's own metadata decides, since the path may name another file by now.
    let opened = file
        .metadata()
        .map_err(|err| Refusal::new(format!("cannot read {shown}: {err}")))?;
    if !opened.is_file() {
        return Err(not_a_file(path));
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .and_then(|_| file.seek(SeekFrom::Start(0)))
        .map_err(|err| Refusal::new(format!("cannot read {shown}: {err}")))?;
    let text = String::from_utf8(bytes)
        .map_err(|_| Refusal::new(format!("{shown} is not UTF-8 text, which TOML must be")))?;
    Ok((file, text))
}

/// The arguments iroh-relay is started with, after the guard has checked the file it hands over
/// as standard input. One argument, `--config-path=/dev/stdin`, and nothing from the command line
/// the guard was given.
///
/// ```
/// assert_eq!(sluice_relay_guard::relay_args(), ["--config-path=/dev/stdin"]);
/// ```
pub fn relay_args() -> Vec<OsString> {
    vec![OsString::from(format!("--config-path={CONFIG_ON_STDIN}"))]
}

fn cannot_open(path: &Path, err: &io::Error) -> Refusal {
    let shown = path.display();
    Refusal::new(match err.kind() {
        io::ErrorKind::NotFound => format!(
            "the config file {shown} does not exist, and iroh-relay would run open with no \
             limit: mount the file there, or fix --config-path"
        ),
        _ => format!("cannot read the config file {shown}: {err}"),
    })
}

fn not_a_file(path: &Path) -> Refusal {
    Refusal::new(format!(
        "{} is not a regular file: point --config-path at the config file itself",
        path.display()
    ))
}
