use std::fs;
use std::path::Path;

use tempfile::TempDir;

mod aliases;
mod junction;
mod owner;
mod restricted;

use super::{INHERIT, canonical_sddl, dacl_sddl, open_dir, private_sddl, set_dacl, user_sid};
use crate::Home;

struct Ace {
    flags: String,
    rights: String,
    trustee: String,
}

/// The access list of `path` as Windows writes it back, so a trustee the system shortens to an
/// alias and the same trustee spelled in full are one text.
fn read(path: &Path) -> (bool, Vec<Ace>) {
    let sddl = canonical_sddl(&dacl_sddl(path).unwrap()).unwrap();
    (is_protected(&sddl), aces_of(&sddl))
}

fn is_protected(sddl: &str) -> bool {
    let (header, _) = sddl.split_once('(').unwrap();
    header.starts_with("D:") && header.contains('P')
}

fn aces_of(sddl: &str) -> Vec<Ace> {
    let (_, entries) = sddl.split_once('(').unwrap();
    entries
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
        .collect()
}

/// The trustees of `sddl`, after the same round trip the product code compares.
fn trustees_of(sddl: &str) -> Vec<String> {
    aces_of(&canonical_sddl(sddl).unwrap())
        .into_iter()
        .map(|ace| ace.trustee)
        .collect()
}

/// The entries are the user, SYSTEM and Administrators with full control, and nothing else.
/// `flags` is what the entries carry: inherit flags on a folder, and `ID` as well where the entry
/// itself came from the folder above. `protected` is whether inheritance from the parent is
/// stopped, which it is on what sluice sets itself and is not on the folders made inside the home.
fn assert_private(path: &Path, flags: &str, protected: bool) {
    let user = user_sid().unwrap();
    assert_private_for(path, flags, protected, &user);
}

/// [`assert_private`] for the user `user`, so a test can check a list built for another account.
fn assert_private_for(path: &Path, flags: &str, protected: bool, user: &str) {
    let (is_protected, aces) = read(path);
    let sddl = dacl_sddl(path).unwrap();
    assert_aces(&sddl, is_protected, &aces, flags, protected, user);
}

/// The comparison itself, on a list already read, so the alias tests can give it text without a
/// folder whose access nobody else may change.
fn assert_aces(
    sddl: &str,
    is_protected: bool,
    aces: &[Ace],
    flags: &str,
    protected: bool,
    user: &str,
) {
    assert_eq!(is_protected, protected, "{sddl}");
    let mut trustees: Vec<String> = aces.iter().map(|ace| ace.trustee.clone()).collect();
    // The wanted list goes through the same round trip, so a user the system writes back as an
    // alias is compared with the alias and not with the SID the list was built from.
    let mut want = trustees_of(&private_sddl(user, ""));
    trustees.sort_unstable();
    want.sort_unstable();
    assert_eq!(trustees, want, "{sddl}");
    for ace in aces {
        assert_eq!(ace.rights, "FA", "{sddl}");
        assert_eq!(ace.flags, flags, "{sddl}");
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
    for name in ["db", "db", "logs"] {
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
fn a_home_that_holds_a_file_keeps_what_it_inherited_and_its_key_is_still_private() {
    let parent = parent_that_lets_everyone_read();
    let path = parent.path().join("home");
    fs::create_dir(&path).unwrap();
    fs::write(path.join("notes.txt"), "mine").unwrap();
    assert!(
        has_everyone(&path),
        "the parent's entry should be inherited"
    );
    let home = Home::open(&path).unwrap();
    assert!(
        has_everyone(&path),
        "a home that holds anything is left as it is"
    );
    home.identity().unwrap();
    assert_private(&path.join("identity.key"), "", true);
    assert_eq!(fs::read_to_string(path.join("notes.txt")).unwrap(), "mine");
}

#[test]
fn an_empty_folder_made_by_hand_gets_the_list_of_a_new_home() {
    let parent = parent_that_lets_everyone_read();
    let path = parent.path().join("home");
    fs::create_dir(&path).unwrap();
    assert!(has_everyone(&path));
    drop(Home::open(&path).unwrap());
    assert_private(&path, INHERIT, true);
    // Made after the list was set, so they inherit it and not the parent's.
    assert_private(&path.join("db"), "OICIID", false);
}

#[test]
fn a_fresh_home_whose_list_cannot_be_changed_is_a_storage_error() {
    let parent = tempfile::tempdir().unwrap();
    let path = parent.path().join("home");
    fs::create_dir(&path).unwrap();
    // `OW` is OWNER RIGHTS: denying it WRITE_DAC takes away the right to change the list that an
    // owner otherwise always has, so no administrator is needed to build this case. An elevated
    // token makes `Administrators` the owner of a folder it creates, and `OW` then names a group
    // and not this user, so the user's own SID is denied as well. The user keeps full control
    // otherwise, so the folder can still be deleted.
    let user = user_sid().unwrap();
    let sddl = format!("D:P(D;;WD;;;OW)(D;;WD;;;{user})(A;OICI;FA;;;{user})");
    set_dacl(&open_dir(&path).unwrap(), &sddl).unwrap();
    let before = dacl_sddl(&path).unwrap();
    // A token with the restore right asks for `WRITE_DAC` through the backup semantics whatever
    // the list says, so no list can build this refusal under it. The no-list case for every token
    // is in `restricted.rs`.
    if open_dir(&path).is_ok() {
        eprintln!(
            "SKIPPED: this token can change the list whatever it says, so the list cannot refuse \
             the open"
        );
        return;
    }
    let Err(crate::Error::Storage(message)) = Home::open(&path) else {
        panic!("expected a storage error");
    };
    assert!(message.contains("restrict access to"), "{message}");
    assert_eq!(
        fs::read_dir(&path).unwrap().count(),
        0,
        "nothing was written"
    );
    assert_eq!(dacl_sddl(&path).unwrap(), before, "the list is as it was");
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
    let subfolder = folder.join("db");
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

#[test]
fn doctor_reads_the_key_list_back_and_fails_on_an_extra_entry() {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{READ_CONTROL, WRITE_DAC};

    use crate::doctor::Finding;

    let parent = parent_that_lets_everyone_read();
    let path = parent.path().join("home");
    drop(Home::open(&path).unwrap().identity().unwrap());
    let key = path.join("identity.key");
    assert!(super::is_private_file(&key).unwrap());
    assert_eq!(key_check(&path), Finding::KeyPrivate);

    // The private list with one more entry: Everyone may read.
    let file = fs::OpenOptions::new()
        .access_mode(READ_CONTROL | WRITE_DAC)
        .open(&key)
        .unwrap();
    let wider = format!("{}(A;;FR;;;WD)", private_sddl(&user_sid().unwrap(), ""));
    set_dacl(&file, &wider).unwrap();
    drop(file);
    assert!(!super::is_private_file(&key).unwrap());
    assert_eq!(key_check(&path), Finding::KeyAclNotPrivate);
}

#[test]
fn the_free_space_of_a_volume_is_a_number_and_a_missing_path_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    assert!(super::free_bytes(dir.path()).unwrap() > 0);
    assert!(super::free_bytes(&dir.path().join("missing")).is_err());
}

/// What check `name` of `sluice doctor` finds in `home`.
fn check_of(home: &Path, name: crate::doctor::Name) -> crate::doctor::Finding {
    crate::doctor::examine_at(home, false)
        .into_iter()
        .find(|check| check.name() == name)
        .unwrap()
        .finding()
        .clone()
}

/// What the key check finds: the key's own list, and not the home's.
fn key_check(home: &Path) -> crate::doctor::Finding {
    check_of(home, crate::doctor::Name::Key)
}

/// What the access check finds: the home folder's list.
fn access_check(home: &Path) -> crate::doctor::Finding {
    check_of(home, crate::doctor::Name::Access)
}

#[test]
fn doctor_passes_a_home_the_store_made() {
    use crate::doctor::Finding;
    let parent = parent_that_lets_everyone_read();
    let path = parent.path().join("home");
    drop(Home::open(&path).unwrap().identity().unwrap());
    assert!(super::is_private_dir(&path).unwrap());
    assert_eq!(access_check(&path), Finding::AccessPrivate);
    assert_eq!(key_check(&path), Finding::KeyPrivate);
}

#[test]
fn doctor_fails_a_home_whose_list_has_an_extra_entry() {
    use crate::doctor::{Check, Finding, Status};
    let parent = tempfile::tempdir().unwrap();
    let path = parent.path().join("home");
    drop(Home::open(&path).unwrap().identity().unwrap());
    // The private list with one more entry: Everyone may read, here and in what is made inside.
    let wider = format!(
        "{}(A;{INHERIT};FR;;;WD)",
        private_sddl(&user_sid().unwrap(), INHERIT)
    );
    set_dacl(&open_dir(&path).unwrap(), &wider).unwrap();
    assert!(!super::is_private_dir(&path).unwrap());
    let finding = access_check(&path);
    assert_eq!(finding, Finding::AccessAclNotPrivate);
    let check = Check::new(finding);
    assert_eq!(check.status(), Status::Failed);
    assert_eq!(check.failure(), Some(crate::ErrorKind::Policy));
}

#[test]
fn doctor_passes_an_empty_folder_made_by_hand_then_opened() {
    use crate::doctor::Finding;
    let parent = parent_that_lets_everyone_read();
    let path = parent.path().join("home");
    fs::create_dir(&path).unwrap();
    drop(Home::open(&path).unwrap().identity().unwrap());
    assert!(super::is_private_dir(&path).unwrap());
    // The access check, which reads the home's list, and not only the key's.
    assert_eq!(access_check(&path), Finding::AccessPrivate);
    assert_eq!(key_check(&path), Finding::KeyPrivate);
}

#[test]
fn doctor_fails_a_folder_made_by_hand_with_a_file_in_it_then_opened() {
    use crate::doctor::Finding;
    let parent = tempfile::tempdir().unwrap();
    let path = parent.path().join("home");
    fs::create_dir(&path).unwrap();
    fs::write(path.join("notes.txt"), "mine").unwrap();
    // The store leaves the list of a folder that holds anything as it is, so it still inherits
    // from the folder above.
    drop(Home::open(&path).unwrap().identity().unwrap());
    assert!(!super::is_private_dir(&path).unwrap());
    assert_eq!(access_check(&path), Finding::AccessAclNotPrivate);
    assert_eq!(key_check(&path), Finding::KeyPrivate);
}

#[test]
fn doctor_reports_a_home_and_a_key_that_are_both_open_as_two_failures() {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{READ_CONTROL, WRITE_DAC};

    use crate::doctor::Finding;

    let parent = tempfile::tempdir().unwrap();
    let path = parent.path().join("home");
    drop(Home::open(&path).unwrap().identity().unwrap());
    let user = user_sid().unwrap();
    let home = format!("{}(A;{INHERIT};FR;;;WD)", private_sddl(&user, INHERIT));
    set_dacl(&open_dir(&path).unwrap(), &home).unwrap();
    let key = fs::OpenOptions::new()
        .access_mode(READ_CONTROL | WRITE_DAC)
        .open(path.join("identity.key"))
        .unwrap();
    set_dacl(&key, &format!("{}(A;;FR;;;WD)", private_sddl(&user, ""))).unwrap();
    drop(key);
    assert_eq!(access_check(&path), Finding::AccessAclNotPrivate);
    assert_eq!(key_check(&path), Finding::KeyAclNotPrivate);
}
