//! Who owns a folder, read through a handle that does not follow a link. An owner can always
//! rewrite the access list, so a home another user made first is refused, and doctor fails it.

use std::path::{Path, PathBuf};

use super::super::{ADMINISTRATORS, is_owned_by_us, user_sid};

#[test]
fn a_folder_this_user_made_is_owned_by_this_user_or_administrators() {
    let dir = tempfile::tempdir().unwrap();
    assert!(is_owned_by_us(dir.path()).unwrap());
    let me = user_sid().unwrap();
    assert!(me == ADMINISTRATORS || !me.is_empty());
}

/// The folder of the system's own files belongs to `TrustedInstaller`, on every Windows, and to no
/// user of the machine. It is the folder of another owner that any test machine has.
#[test]
fn a_folder_of_the_system_is_owned_by_someone_else() {
    let root = std::env::var_os("SystemRoot").expect("Windows sets SystemRoot");
    let system: PathBuf = Path::new(&root).join("System32");
    assert!(!is_owned_by_us(&system).unwrap());
}

/// The path of a verbatim form is read as it is, which `Home` gives it.
#[test]
fn the_owner_is_read_through_a_verbatim_path() {
    let dir = tempfile::tempdir().unwrap();
    let verbatim = std::fs::canonicalize(dir.path()).unwrap();
    assert!(verbatim.display().to_string().starts_with(r"\\?\"));
    assert!(is_owned_by_us(&verbatim).unwrap());
}
