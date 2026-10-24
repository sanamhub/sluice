//! Every open checks who owns the home and each folder of its layout. The owner is a parameter,
//! because a folder of another user cannot be made without being root.

use std::io;
use std::path::Path;

use super::super::{FOLDERS, prepare_with};
use crate::Error;

/// Not only a home that is new: a folder another user swapped in is theirs to change after this
/// one looks away, so the check runs on every open.
#[test]
fn the_home_and_every_folder_of_it_are_checked_for_their_owner_on_every_open() {
    fn ours(_: &Path) -> io::Result<bool> {
        Ok(true)
    }
    fn blobs_not_ours(path: &Path) -> io::Result<bool> {
        Ok(!path.ends_with("blobs"))
    }
    let dir = tempfile::tempdir().unwrap();
    let Err(Error::Policy(message)) = prepare_with(dir.path(), false, |_| Ok(()), blobs_not_ours)
    else {
        panic!("a blobs folder another user owns was opened");
    };
    assert!(message.contains("blobs"), "{message}");
    assert!(message.contains("belongs to another user"), "{message}");
    // The home itself, before anything in it is looked at.
    let dir = tempfile::tempdir().unwrap();
    let Err(Error::Policy(message)) = prepare_with(dir.path(), false, |_| Ok(()), |_| Ok(false))
    else {
        panic!("a home another user owns was opened");
    };
    assert!(
        message.contains(&dir.path().display().to_string()),
        "{message}"
    );
    // With every entry this user's it opens, and makes the folders it always made.
    let dir = tempfile::tempdir().unwrap();
    prepare_with(dir.path(), false, |_| Ok(()), ours).unwrap();
    for name in FOLDERS {
        assert!(dir.path().join(name).is_dir(), "{name}");
    }
}
