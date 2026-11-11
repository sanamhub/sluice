//! The one note a removal leaves: the folder it gave up on and which folder that was, so the next
//! claim of that output takes away that folder and not one that took its name since.

use std::{fs, path::Path};

use super::super::{FolderId, identify};
use super::canonical;

/// The start of every note's name in the records folder, which tells a note from a record there: a
/// record is named by a random id, and a note by this and the hash of the folder's path.
pub(crate) const PREFIX: &str = "gone-";

/// The name of the note that stands for `path`'s folder in the records folder: a fixed prefix and
/// the hash of the one form of the path. The note holds the folder's identity and nothing else, so
/// no text in it can name another path, and a note is honoured only for the folder it was made for.
fn note_name(path: &Path) -> Option<String> {
    let text = path.to_str()?;
    Some(format!(
        "{PREFIX}{}",
        blake3::hash(text.as_bytes()).to_hex()
    ))
}

/// The identity the note for `path`'s folder holds, if a removal left one.
///
/// A note is a claim on one folder, and a folder that took its name is another folder: the answer
/// is `None` when the note is absent, when it cannot be read, and when its text is not an identity,
/// and the caller then treats the name as one no note explains.
pub(crate) fn noted(records: Option<&Path>, path: &Path) -> Option<FolderId> {
    let dir = records?;
    let wanted = canonical(path).ok()?;
    let text = fs::read_to_string(dir.join(note_name(&wanted)?)).ok()?;
    FolderId::from_note(text.trim_end_matches(['\r', '\n']))
}

/// Notes that a removal gave up on `path`'s folder, with the identity that folder has now.
///
/// The record of the store it was went with the attempt, and a folder with the name of a partial
/// store and no record is refused, so without this note a receive into the same output would fail
/// until the user removed the folder by hand. A folder whose identity cannot be read is not noted:
/// a note that does not name the folder it stands for is one a later claim could honour for a
/// folder another command made. Best effort: a note that cannot be written leaves the folder
/// refused as it was.
pub(crate) fn note(records: Option<&Path>, path: &Path) {
    let Some(dir) = records else {
        return;
    };
    let Ok(Some(identity)) = identify(path) else {
        return;
    };
    let Ok(wanted) = canonical(path) else {
        return;
    };
    let Some(name) = note_name(&wanted) else {
        return;
    };
    let _ = fs::write(dir.join(name), identity.to_note());
}

/// Removes the note that stands for `path`'s folder, after that folder is gone.
pub(crate) fn forget(records: Option<&Path>, path: &Path) {
    // Only a folder named like a partial store can have a note, and a removal forgets a note for
    // every folder it takes, most of which are temporary folders with no note at all.
    if !super::super::store::is_partial(path) {
        return;
    }
    let Some(dir) = records else {
        return;
    };
    let Ok(wanted) = canonical(path) else {
        return;
    };
    let Some(name) = note_name(&wanted) else {
        return;
    };
    let _ = fs::remove_file(dir.join(name));
}
