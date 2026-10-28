//! The home's record of the partial stores a receive made. A folder named `.sluice-partial-*` beside
//! an output folder belongs to a transfer only when a record here names it, its absolute path and
//! the ticket root. A folder with that name and no record is someone else's: it is refused unread
//! and never removed.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use super::remove_quietly;
use crate::{Error, Home};

/// A record of one partial store.
pub struct Record {
    /// The random id, which is the record's file name and a line in the store's marker.
    pub id: String,
    /// The absolute folder the store lives in.
    pub path: PathBuf,
    /// The ticket root, as the marker spells it.
    pub root: String,
}

/// Writes the record of a store this receive has just made and returns its id. The id is also
/// written into the store's marker, so a folder that was replaced under the same name is not taken
/// for this one.
pub fn remember(path: &Path, root: &str) -> Result<String, Error> {
    let dir = Home::partial_records()?;
    let id = random_id()?;
    let text = format!("path {}\nroot {root}\n", canonical(path).display());
    fs::write(dir.join(&id), text).map_err(|e| storage("write a record in", &dir, e))?;
    Ok(id)
}

/// The record naming `path` and `root`, if this home has one.
pub fn find(path: &Path, root: &str) -> Result<Option<Record>, Error> {
    let dir = Home::partial_records()?;
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(storage("read", &dir, e)),
    };
    let wanted = canonical(path);
    for entry in entries.flatten() {
        let Some(id) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        let text = match fs::read_to_string(entry.path()) {
            Ok(text) => text,
            // A record another process is writing, or one being removed, is not this one.
            Err(_) => continue,
        };
        let Some(record) = parse(&id, &text) else {
            continue;
        };
        if record.path == wanted && record.root == root {
            return Ok(Some(record));
        }
    }
    Ok(None)
}

/// Removes the record of `path`, if there is one. Best effort: a store that is gone is not worth a
/// second error.
pub fn forget(path: &Path) {
    let Ok(dir) = Home::partial_records() else {
        return;
    };
    let wanted = canonical(path);
    let Ok(entries) = fs::read_dir(&dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Some(id) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        let Ok(text) = fs::read_to_string(entry.path()) else {
            continue;
        };
        if parse(&id, &text).is_some_and(|record| record.path == wanted) {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// Removes a partial store and the home's record of it, which are both this receive's.
pub fn discard(path: &Path) {
    remove_quietly(path);
    forget(path);
}

/// [`discard`] for an async caller. The removal waits for a file another process may still hold,
/// and those waits belong on a blocking thread, not on the thread the caller runs on.
pub async fn discard_apart(path: &Path) {
    let path = path.to_owned();
    let _ = tokio::task::spawn_blocking(move || discard(&path)).await;
}

/// The error for a `.sluice-partial-*` folder that no record of this home names.
pub fn clash(path: &Path) -> Error {
    Error::Policy(format!(
        "{} is named like a partial store, and no record in this home says it is one; remove it, or \
         pass another --out",
        path.display()
    ))
}

/// Records `path` under a given id, which a test uses to make the id a marker carries and the root
/// a record names belong to two different transfers.
#[cfg(test)]
pub(crate) fn remember_as(path: &Path, root: &str, id: &str) -> Result<(), Error> {
    let dir = Home::partial_records()?;
    let text = format!("path {}\nroot {root}\n", canonical(path).display());
    fs::write(dir.join(id), text).map_err(|e| storage("write a record in", &dir, e))
}

fn parse(id: &str, text: &str) -> Option<Record> {
    let mut path = None;
    let mut root = None;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("path ") {
            path = Some(PathBuf::from(rest));
        } else if let Some(rest) = line.strip_prefix("root ") {
            root = Some(rest.to_string());
        }
    }
    Some(Record {
        id: id.to_string(),
        path: path.filter(|path| path.is_absolute())?,
        root: root?,
    })
}

/// The form of `path` a record holds and is compared against. The folder that holds the store is
/// resolved once, so the same store is recognized however its path was spelled, whether with a case
/// Windows folds, a `\\?\` prefix, a `..` or a link in the output folder. Only the parent is
/// resolved: the store itself may not exist yet, and a name that was swapped under the same path is
/// caught by the marker's id rather than by this text.
pub(super) fn canonical(path: &Path) -> PathBuf {
    let Some(name) = path.file_name() else {
        return absolute(path);
    };
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    match fs::canonicalize(parent) {
        Ok(real) => real.join(name),
        Err(_) => absolute(path),
    }
}

fn absolute(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

fn random_id() -> Result<String, Error> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|e| {
        Error::Storage(format!(
            "cannot get a random name for a partial-store record: {e}"
        ))
    })?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

fn storage(doing: &str, path: &Path, source: io::Error) -> Error {
    Error::Storage(format!("cannot {doing} {}: {source}", path.display()))
}
