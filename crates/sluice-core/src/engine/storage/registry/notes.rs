//! The one note a removal leaves: the folder it gave up on, so the next claim of that output takes
//! the folder away instead of refusing a name no record explains.

use std::{fs, path::Path};

use super::canonical;
use crate::Home;

/// The name of the note that stands for `path`'s folder in the records folder: a fixed prefix and
/// the hash of the one form of the path. The note holds nothing, so no text in it can name another
/// path, and a note is honoured only for the folder it was made for.
fn note_name(path: &Path) -> Option<String> {
    let text = path.to_str()?;
    Some(format!("gone-{}", blake3::hash(text.as_bytes()).to_hex()))
}

/// Notes that a removal gave up on `path`'s folder. The record of the store it was went with the
/// attempt, and a folder with the name of a partial store and no record is refused, so without this
/// note a receive into the same output would fail until the user removed the folder by hand. Best
/// effort: a note that cannot be written leaves the folder refused as it was.
pub(crate) fn note_discarded(path: &Path) {
    let Ok(wanted) = canonical(path) else {
        return;
    };
    let Some(name) = note_name(&wanted) else {
        return;
    };
    let Ok(dir) = Home::partial_records() else {
        return;
    };
    let _ = fs::write(dir.join(name), b"");
}

/// Whether a removal gave up on `path`'s folder and no claim has taken the note yet.
pub(crate) fn discarded(path: &Path) -> bool {
    let Some(dir) = Home::partial_records_for_read() else {
        return false;
    };
    let Ok(wanted) = canonical(path) else {
        return false;
    };
    note_name(&wanted).is_some_and(|name| dir.join(name).exists())
}

/// Removes the note that stands for `path`'s folder, after the folder it stands for is gone.
pub(crate) fn forget_discarded(path: &Path) {
    let Ok(wanted) = canonical(path) else {
        return;
    };
    let Some(name) = note_name(&wanted) else {
        return;
    };
    let Some(dir) = Home::partial_records_for_read() else {
        return;
    };
    let _ = fs::remove_file(dir.join(name));
}
