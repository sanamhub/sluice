//! Keeping the receipt out of the output folder. The receipt is written after the content, so a
//! path the receive writes into would be replaced, or refused, only once everything had arrived.
//! The rule is the whole folder, not the files the manifest names: a receipt path that is `--out`
//! or inside it is refused, and it is refused before anything is created.
//!
//! A path is compared twice. The first time as the text says it, with `.` and `..` folded and case
//! ignored. The second time after the folders that exist have been resolved, so a link, a junction,
//! a short name or a verbatim (`\\?\`) spelling that reaches the same folder is the same path. A
//! refusal that is too wide costs a different file name; one that is too narrow loses a received
//! file.

use std::path::{Component, Path, PathBuf};

use crate::Error;
use crate::manifest::path::shown;

#[cfg(test)]
mod tests;

/// Refuses a receipt `path` that is `out` or inside it, in either comparison.
pub(in crate::engine) fn refuse_under_out(receipt: &Path, out: &Path) -> Result<(), Error> {
    let written = key(receipt).starts_with(key(out));
    if written || resolved_receipt(receipt).starts_with(resolved(out)) {
        return Err(Error::Policy(format!(
            "{} is --out or inside it ({}), and the receive writes there, so the receipt cannot \
             go there; give a path outside --out",
            shown(&receipt.display().to_string()),
            shown(&out.display().to_string())
        )));
    }
    Ok(())
}

/// The receipt's own path with the folder it goes into resolved. The name itself is not followed:
/// writing the receipt replaces a link at the name and never writes through it.
fn resolved_receipt(receipt: &Path) -> PathBuf {
    let parent = match receipt.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    let mut at = resolved(parent);
    if let Some(name) = receipt.file_name() {
        at.push(name.to_string_lossy().to_lowercase());
    }
    at
}

/// `path` with the part that exists resolved by the file system and the rest, which does not exist
/// yet, folded as text. The resolved part is the deepest ancestor that can be resolved.
fn resolved(path: &Path) -> PathBuf {
    let mut rest = Vec::new();
    let mut at = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    loop {
        if let Ok(real) = at.canonicalize() {
            let mut whole = key(&real);
            whole.extend(rest.iter().rev());
            return whole;
        }
        let Some(name) = at
            .file_name()
            .map(|name| name.to_string_lossy().to_lowercase())
        else {
            return key(path);
        };
        rest.push(name);
        if !at.pop() {
            return key(path);
        }
    }
}

/// A path as a comparison key: absolute, `.` and `..` folded away lexically, lower case.
fn key(path: &Path) -> PathBuf {
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let mut folded = PathBuf::new();
    for part in absolute.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                folded.pop();
            }
            other => folded.push(other.as_os_str().to_string_lossy().to_lowercase()),
        }
    }
    folded
}
