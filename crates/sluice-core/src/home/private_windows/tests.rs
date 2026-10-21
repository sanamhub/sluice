use std::fs;
use std::path::Path;

use tempfile::TempDir;

use super::{INHERIT, dacl_sddl, open_dir, private_sddl, set_dacl, user_sid};
use crate::Home;

struct Ace {
    flags: String,
    rights: String,
    trustee: String,
}

/// The access list of `path`: whether it is protected from inheritance, and its entries.
fn read(path: &Path) -> (bool, Vec<Ace>) {
    let sddl = dacl_sddl(path).unwrap();
    let (header, entries) = sddl.split_once('(').unwrap();
    let aces = entries
        .trim_end_matches(')')
        .split(")(")
        .map(|entry| {
            // type;flags;rights;object guid;inherited object guid;trustee
            let fields: Vec<&str> = entry.split(';').collect();
            assert_eq!(fields.len(), 6, "{sddl}");
            assert_eq!(fields[0], "A", "{sddl}");
            Ace {
                flags: fields[1].to_string(),
                rights: fields[2].to_string(),
                trustee: fields[5].to_string(),
            }
        })
        .collect();
    (header.starts_with("D:") && header.contains('P'), aces)
}

/// The entries are the user, SYSTEM and Administrators with full control, and nothing else.
/// `flags` is what the entries carry: inherit flags on a folder, and `ID` as well where the entry
/// itself came from the folder above. `protected` is whether inheritance from the parent is
/// stopped, which it is on what sluice sets itself and is not on the folders made inside the home.
fn assert_private(path: &Path, flags: &str, protected: bool) {
    let (is_protected, aces) = read(path);
    let sddl = dacl_sddl(path).unwrap();
    assert_eq!(is_protected, protected, "{}: {sddl}", path.display());
    let user = user_sid().unwrap();
    let mut trustees: Vec<&str> = aces.iter().map(|ace| ace.trustee.as_str()).collect();
    trustees.sort_unstable();
    let mut want = vec![user.as_str(), "SY", "BA"];
    want.sort_unstable();
    assert_eq!(trustees, want, "{}: {sddl}", path.display());
    for ace in &aces {
        assert_eq!(ace.rights, "FA", "{}: {sddl}", path.display());
        assert_eq!(ace.flags, flags, "{}: {sddl}", path.display());
    }
}

fn has_everyone(path: &Path) -> bool {
    read(path).1.iter().any(|ace| ace.trustee == "WD")
}

/// A folder whose access list lets `Everyone` read what is made inside it, which is what a
/// parent that is looser than a stock profile looks like.
fn parent_that_lets_everyone_read() -> TempDir {
    let parent = tempfile::tempdir().unwrap();
    let sddl = format!(
        "{}(A;{INHERIT};FR;;;WD)",
        private_sddl(&user_sid().unwrap(), INHERIT)
    );
    set_dacl(&open_dir(parent.path()).unwrap(), &sddl).unwrap();
    assert!(has_everyone(parent.path()));
    parent
}

#[test]
fn a_new_home_and_its_folders_have_the_three_entries_and_no_everyone() {
    let parent = parent_that_lets_everyone_read();
    let path = parent.path().join("home");
    drop(Home::open(&path).unwrap());
    assert_private(&path, INHERIT, true);
    for name in ["blobs", "db", "logs"] {
        // Inherited from the home, so the entries say so and the list is not protected itself.
        assert_private(&path.join(name), "OICIID", false);
    }
}

#[test]
fn the_key_has_the_three_entries_after_identity_and_after_rotate() {
    let parent = parent_that_lets_everyone_read();
    let path = parent.path().join("home");
    let home = Home::open(&path).unwrap();
    let key = path.join("identity.key");
    home.identity().unwrap();
    assert_private(&key, "", true);
    home.rotate_identity().unwrap();
    assert_private(&key, "", true);
}

#[test]
fn a_home_that_was_already_there_keeps_what_it_inherited_and_its_key_is_still_private() {
    let parent = parent_that_lets_everyone_read();
    let path = parent.path().join("home");
    fs::create_dir(&path).unwrap();
    assert!(
        has_everyone(&path),
        "the parent's entry should be inherited"
    );
    let home = Home::open(&path).unwrap();
    assert!(has_everyone(&path), "an existing home is left as it is");
    home.identity().unwrap();
    assert_private(&path.join("identity.key"), "", true);
}

#[test]
fn the_process_that_made_the_key_can_read_it_again() {
    let parent = parent_that_lets_everyone_read();
    let path = parent.path().join("home");
    let home = Home::open(&path).unwrap();
    let first = home.identity().unwrap().endpoint_id();
    drop(home);
    let again = Home::open(&path).unwrap();
    assert_eq!(again.identity().unwrap().endpoint_id(), first);
}

#[test]
fn a_folder_that_is_not_there_is_an_error_and_not_a_panic() {
    let parent = tempfile::tempdir().unwrap();
    let error = super::restrict_dir(&parent.path().join("missing")).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
}

#[test]
fn the_list_reaches_children_that_were_made_before_it_was_set() {
    let parent = parent_that_lets_everyone_read();
    let folder = parent.path().join("home");
    let subfolder = folder.join("blobs");
    let file = subfolder.join("data");
    fs::create_dir_all(&subfolder).unwrap();
    fs::write(&file, b"x").unwrap();
    // The children took the parent's `Everyone` entry when they were made.
    assert!(has_everyone(&subfolder) && has_everyone(&file));

    super::restrict_dir(&folder).unwrap();

    // Propagation is what a second process relies on when it makes a subfolder in a new home
    // before the first process has set the list.
    assert_private(&folder, INHERIT, true);
    assert_private(&subfolder, "OICIID", false);
    // On a file the inherit flags are dropped and only `ID` is left.
    assert_private(&file, "ID", false);
}
