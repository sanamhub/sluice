//! A junction has an access list of its own. Read by the junction's path, with
//! `GetNamedSecurityInfoW`, Windows gives that list and not its target's, and a handle opened with
//! `FILE_FLAG_OPEN_REPARSE_POINT` changes it alone. Before the home was resolved, `Home::open` on a
//! junction so made the junction private and left the target as it was, and doctor, reading the
//! junction, passed it.

use std::fs;
use std::path::Path;

use super::super::{is_private_dir, restrict_dir};
use super::{access_check, parent_that_lets_everyone_read};
use crate::doctor::Finding;

fn junction(target: &Path, link: &Path) {
    let status = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .stdout(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(status.success(), "mklink /J failed");
}

#[test]
fn a_junction_has_a_list_of_its_own_and_doctor_reads_its_targets() {
    let parent = parent_that_lets_everyone_read();
    let target = parent.path().join("target");
    let link = parent.path().join("home");
    fs::create_dir(&target).unwrap();
    junction(&target, &link);
    // What `Home::open` did through a junction before the home was resolved.
    restrict_dir(&link).unwrap();
    assert!(is_private_dir(&link).unwrap(), "the junction's own list");
    assert!(!is_private_dir(&target).unwrap(), "the target is untouched");
    fs::create_dir(target.join("blobs")).unwrap();
    fs::create_dir(target.join("db")).unwrap();
    fs::create_dir(target.join("logs")).unwrap();
    assert_eq!(access_check(&link), Finding::HomeAclNotPrivate);
    restrict_dir(&target).unwrap();
    assert_eq!(access_check(&link), Finding::AccessPrivate);
}
