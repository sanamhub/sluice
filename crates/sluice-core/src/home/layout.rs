//! The folders of a home and the `VERSION` file that says how they are laid out.

use std::fs::{self, DirBuilder};
use std::io;
use std::path::Path;

use super::atomic::write_atomic;
use super::storage;
use crate::Error;

/// The folders `open` creates under the home, besides the home itself.
const FOLDERS: [&str; 3] = ["blobs", "db", "logs"];

/// A step that takes a home from one layout version to the next.
type Step = fn(&Path) -> Result<(), Error>;

/// `MIGRATIONS[i]` takes layout version `i + 1` to `i + 2`. Empty while there is only version 1.
/// The list only ever grows at the end, because a home is only ever moved forward.
const MIGRATIONS: &[Step] = &[];

/// The layout version that `steps` lead to. A new home is written at this version.
fn latest(steps: &[Step]) -> u32 {
    u32::try_from(steps.len() + 1).unwrap_or(u32::MAX)
}

/// Creates the home and its folders if they are missing. On Unix they get mode 0700, so the
/// identity key is not readable through the folder even if its own mode were loosened by hand.
/// A folder that already exists is left as it is.
pub(super) fn create_folders(home: &Path) -> Result<(), Error> {
    let mut builder = DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(home)
        .map_err(|e| storage("create the folder", home, e))?;
    for name in FOLDERS {
        let folder = home.join(name);
        builder
            .create(&folder)
            .map_err(|e| storage("create the folder", &folder, e))?;
    }
    Ok(())
}

/// The version in `VERSION`, or `None` if there is no such file. Reads only.
pub(super) fn read_version(home: &Path) -> Result<Option<u32>, Error> {
    let path = home.join("VERSION");
    match fs::read_to_string(&path) {
        Ok(text) => parse_version(&text, &path).map(Some),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(storage("read", &path, e)),
    }
}

fn parse_version(text: &str, path: &Path) -> Result<u32, Error> {
    let digits = text.trim_end_matches(['\n', '\r']);
    // Digits only and at most 9 of them: `+1` and ` 1` are not what this file ever holds, and a
    // longer number cannot be a layout version.
    let plain =
        !digits.is_empty() && digits.len() <= 9 && digits.bytes().all(|b| b.is_ascii_digit());
    match digits.parse::<u32>() {
        Ok(version) if plain && version >= 1 => Ok(version),
        _ => Err(Error::Storage(format!(
            "{} does not hold a layout version: expected a decimal number from 1 and a newline",
            path.display()
        ))),
    }
}

/// Refuses a layout newer than this build knows. Reads only, so it runs before anything is
/// created: a newer sluice's home stays exactly as it was.
pub(super) fn refuse_newer(found: u32) -> Result<(), Error> {
    refuse_newer_than(found, latest(MIGRATIONS))
}

fn refuse_newer_than(found: u32, known: u32) -> Result<(), Error> {
    if found > known {
        return Err(Error::Policy(format!(
            "the store has layout version {found} and this sluice reads up to version {known}. \
             It does not write to a newer layout"
        )));
    }
    Ok(())
}

/// With the lock held: writes `VERSION` for a new home, runs the migrations a lower version
/// needs, and refuses one that became newer since `refuse_newer` looked.
pub(super) fn settle(home: &Path) -> Result<(), Error> {
    settle_with(home, MIGRATIONS)
}

fn settle_with(home: &Path, steps: &[Step]) -> Result<(), Error> {
    let known = latest(steps);
    match read_version(home)? {
        None => write_version(home, known),
        Some(found) => {
            refuse_newer_than(found, known)?;
            for version in found..known {
                steps[version as usize - 1](home)?;
                // After each step, so a crash between two steps resumes at the next one.
                write_version(home, version + 1)?;
            }
            Ok(())
        }
    }
}

fn write_version(home: &Path, version: u32) -> Result<(), Error> {
    write_atomic(home, "VERSION", format!("{version}\n").as_bytes())
}

#[cfg(test)]
mod tests;
