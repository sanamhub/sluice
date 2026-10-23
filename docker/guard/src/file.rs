//! The config file, opened once. `handover.rs` says how iroh-relay reads the sealed copy.

use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

use crate::Refusal;

/// Opens the config file and reads it. The file must exist and be a regular file: iroh-relay runs
/// its open default when the path does not exist. It returns the open file, at its start, with
/// the text read from it, so the handover can copy the bytes that were checked.
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
    // The path first, so that a FIFO or a device is refused before an open that could block.
    let named = fs::metadata(path).map_err(|err| cannot_open(path, &err))?;
    if !named.is_file() {
        return Err(not_a_file(path));
    }
    let mut file = File::open(path).map_err(|err| cannot_open(path, &err))?;
    // The open file's own metadata decides, since the path may name another file by now.
    let opened = file
        .metadata()
        .map_err(|err| Refusal::new(format!("cannot read {}: {err}", shown(path))))?;
    if !opened.is_file() {
        return Err(not_a_file(path));
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .and_then(|_| file.seek(SeekFrom::Start(0)))
        .map_err(|err| Refusal::new(format!("cannot read {}: {err}", shown(path))))?;
    let text = String::from_utf8(bytes).map_err(|_| {
        Refusal::new(format!(
            "{} is not UTF-8 text, which TOML must be",
            shown(path)
        ))
    })?;
    Ok((file, text))
}

/// How the value of `--config-path` is named in a refusal. It is text from the command line and may
/// be a token pasted in the wrong place, so it is printed only when it cannot be one: a path that
/// starts with `/` or `./`, is at most 200 characters, and uses only `[A-Za-z0-9._/-]`. Any other
/// is "the path given to --config-path". A path that passes can still be a token, but then it is
/// indistinguishable from a path, and the operator has to read it.
fn shown(path: &Path) -> String {
    const UNNAMED: &str = "the path given to --config-path";
    let Some(text) = path.to_str() else {
        return UNNAMED.to_string();
    };
    let plain = |b: u8| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'/' | b'-');
    let printable = (text.starts_with('/') || text.starts_with("./"))
        && text.len() <= 200
        && text.bytes().all(plain);
    match printable {
        true => text.to_string(),
        false => UNNAMED.to_string(),
    }
}

fn cannot_open(path: &Path, err: &io::Error) -> Refusal {
    let shown = shown(path);
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
        shown(path)
    ))
}
