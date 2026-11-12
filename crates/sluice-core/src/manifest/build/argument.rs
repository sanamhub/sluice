//! The top-level arguments of a send: the name an argument contributes, and a file argument as an
//! entry of its own.

use std::fs;
use std::path::Path;

use unicode_normalization::UnicodeNormalization;

use super::walk::{Item, What};
use crate::Error;
use crate::manifest::path::{ManifestPath, component_rule};

/// The top-level name, refused when another argument already carries it.
pub(super) fn unique_name(
    names: &mut Vec<(String, String)>,
    name: String,
    shown: &str,
) -> Result<String, Error> {
    if let Some((_, other)) = names.iter().find(|(existing, _)| existing == &name) {
        return Err(Error::Policy(format!(
            "{other} and {shown} would both arrive as {name}; rename one of them or send \
             them in separate transfers"
        )));
    }
    names.push((name.clone(), shown.to_string()));
    Ok(name)
}

/// One file argument as a top-level entry. A link is refused: its content lives somewhere else.
pub(super) fn file_item(
    shown: &str,
    name: &str,
    before: &fs::Metadata,
    path: &Path,
) -> Result<Item, Error> {
    if before.file_type().is_symlink() {
        return Err(Error::Policy(format!(
            "{shown} is a symlink; send the file it points at by its real path"
        )));
    }
    if !before.is_file() {
        return Err(Error::Policy(format!(
            "{shown} is not a regular file or a folder"
        )));
    }
    let entry_path = ManifestPath::parse_rule(name).map_err(|rule| {
        Error::Policy(format!(
            "{shown} has a name a receiver would reject (rule {rule})"
        ))
    })?;
    Ok(Item {
        path: entry_path,
        shown: name.to_string(),
        what: What::File {
            disk: path.to_path_buf(),
            id: None,
        },
    })
}

/// The name as the manifest carries it, after the checks a receiver applies to every component.
pub(super) fn nfc_name(name: &str) -> Result<String, Error> {
    let nfc: String = name.nfc().collect();
    match component_rule(&nfc) {
        None => Ok(nfc),
        Some(rule) => Err(Error::Policy(format!(
            "a path to send has a name a receiver would reject (rule {rule})"
        ))),
    }
}
