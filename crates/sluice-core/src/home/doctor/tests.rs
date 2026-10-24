use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

mod keys;
mod links;

use super::checks::classify_layout;
use super::{Check, Finding, LOW_SPACE_BYTES, Location, Name, Status, examine_at};
use crate::{Error, ErrorKind, Home};

/// A home that the store made inside a temporary folder. The folder itself is not the home,
/// because a temporary folder can have mode 0755, and the key check refuses a home that others can
/// reach.
struct TestHome {
    _dir: tempfile::TempDir,
    home: PathBuf,
}

impl TestHome {
    fn path(&self) -> &Path {
        &self.home
    }
}

/// A home with no key yet, and the lock let go.
fn empty_home() -> TestHome {
    let dir = tempfile::tempdir().expect("a temporary folder");
    let home = dir.path().join("home");
    drop(Home::open(&home).expect("a home"));
    TestHome { _dir: dir, home }
}

/// A home made the way `sluice id` makes one: folders, `VERSION`, key, and the lock let go.
fn ready_home() -> TestHome {
    let home = empty_home();
    Home::open(home.path())
        .expect("a home")
        .identity()
        .expect("a key");
    home
}

fn finding(home: &Path, name: Name) -> Finding {
    let checks = examine_at(home, false);
    let check = checks
        .iter()
        .find(|check| check.name() == name)
        .expect("every check is in the report");
    check.finding().clone()
}

/// Every file under `dir` and its bytes, to show that a doctor run wrote nothing.
fn contents(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut found = BTreeMap::new();
    for entry in fs::read_dir(dir).expect("a folder") {
        let path = entry.expect("an entry").path();
        let name = path.display().to_string();
        if path.is_dir() {
            found.insert(name, Vec::new());
            found.extend(contents(&path));
        } else {
            found.insert(name, fs::read(&path).unwrap_or_default());
        }
    }
    found
}

#[test]
fn a_ready_home_is_ok_from_end_to_end_and_says_where_it_came_from() {
    let dir = ready_home();
    let checks = examine_at(dir.path(), true);
    let location = Location {
        path: dir.path().to_path_buf(),
        from_env: true,
    };
    assert_eq!(checks[0].finding(), &Finding::HomeReady(location));
    let names: Vec<Name> = checks.iter().map(Check::name).collect();
    assert_eq!(
        names,
        [
            Name::Home,
            Name::Access,
            Name::Layout,
            Name::Lock,
            Name::Key,
            Name::Space
        ]
    );
    assert_eq!(checks[1].finding(), &Finding::AccessPrivate);
    for check in &checks[..5] {
        assert_eq!(check.status(), Status::Ok, "{check:?}");
    }
    assert!(matches!(checks[5].finding(), Finding::SpaceFree(_)));
}

#[test]
fn a_missing_home_fails_skips_the_rest_and_is_not_created() {
    let dir = tempfile::tempdir().expect("a temporary folder");
    let path = dir.path().join("home");
    let checks = examine_at(&path, false);
    assert!(matches!(checks[0].finding(), Finding::HomeMissing(_)));
    assert_eq!(checks[0].failure(), Some(ErrorKind::Storage));
    for (check, name) in checks[1..].iter().zip([
        Name::Access,
        Name::Layout,
        Name::Lock,
        Name::Key,
        Name::Space,
    ]) {
        assert_eq!(check.finding(), &Finding::NeedsHome(name));
        assert_eq!(check.status(), Status::Skipped);
    }
    assert!(!path.exists());
}

#[test]
fn a_home_that_is_a_file_fails() {
    let dir = tempfile::tempdir().expect("a temporary folder");
    let path = dir.path().join("home");
    fs::write(&path, b"not a folder").expect("write");
    let checks = examine_at(&path, false);
    assert!(matches!(checks[0].finding(), Finding::HomeNotAFolder(_)));
    assert_eq!(checks[0].status(), Status::Failed);
    assert_eq!(checks[1].status(), Status::Skipped);
}

#[test]
fn a_folder_missing_from_the_layout_fails_and_is_named_and_the_rest_still_run() {
    let dir = ready_home();
    fs::remove_dir(dir.path().join("logs")).expect("remove");
    let checks = examine_at(dir.path(), false);
    let Finding::HomeFoldersMissing(_, missing) = checks[0].finding() else {
        panic!("{:?}", checks[0]);
    };
    assert_eq!(missing, &["logs"]);
    assert_eq!(checks[0].failure(), Some(ErrorKind::Storage));
    assert_eq!(checks[4].finding(), &Finding::KeyPrivate);
}

#[test]
fn the_layout_version_current_older_newer_unreadable_and_missing() {
    assert_eq!(classify_layout(Ok(Some(3)), 3), Finding::LayoutCurrent(3));
    let older = classify_layout(Ok(Some(2)), 3);
    assert_eq!(
        older,
        Finding::LayoutOlder {
            found: 2,
            current: 3
        }
    );
    assert_eq!(Check::new(older).status(), Status::Warning);
    let error = Err(Error::Storage("unreadable".into()));
    let unreadable = classify_layout(error, 3);
    assert!(matches!(unreadable, Finding::LayoutUnreadable(m) if m.contains("unreadable")));

    let dir = ready_home();
    assert_eq!(finding(dir.path(), Name::Layout), Finding::LayoutCurrent(1));
    fs::write(dir.path().join("VERSION"), "2\n").expect("write");
    let newer = finding(dir.path(), Name::Layout);
    assert_eq!(newer, Finding::LayoutNewer { found: 2, known: 1 });
    assert_eq!(Check::new(newer).failure(), Some(ErrorKind::Policy));
    fs::write(dir.path().join("VERSION"), "one\n").expect("write");
    let unreadable = finding(dir.path(), Name::Layout);
    assert!(
        matches!(unreadable, Finding::LayoutUnreadable(_)),
        "{unreadable:?}"
    );
    assert_eq!(Check::new(unreadable).failure(), Some(ErrorKind::Storage));
    fs::remove_file(dir.path().join("VERSION")).expect("remove");
    let none = finding(dir.path(), Name::Layout);
    assert_eq!(none, Finding::LayoutNoFile(1));
    assert_eq!(Check::new(none).status(), Status::Warning);
}

#[test]
fn the_lock_held_free_and_stale_and_a_try_never_keeps_it() {
    let dir = ready_home();
    assert_eq!(finding(dir.path(), Name::Lock), Finding::LockFree);
    let home = Home::open(dir.path()).expect("open");
    let held = finding(dir.path(), Name::Lock);
    assert_eq!(held, Finding::LockHeld(Some(std::process::id())));
    assert_eq!(Check::new(held).status(), Status::Ok);
    drop(home);
    // A process that dies leaves its pid behind.
    fs::write(dir.path().join("lock.pid"), "4242\n").expect("write");
    let stale = finding(dir.path(), Name::Lock);
    assert_eq!(stale, Finding::LockStale(Some(4242)));
    assert_eq!(Check::new(stale).status(), Status::Warning);
    // The try let go at once: the store opens.
    drop(Home::open(dir.path()).expect("the lock is free after a doctor run"));
}

#[test]
fn a_lock_file_that_is_not_there_is_free_and_is_not_created() {
    let dir = ready_home();
    fs::remove_file(dir.path().join("lock")).expect("remove");
    assert_eq!(finding(dir.path(), Name::Lock), Finding::LockFree);
    assert!(!dir.path().join("lock").exists());
}

#[test]
fn the_key_missing_short_and_private() {
    let dir = empty_home();
    let missing = finding(dir.path(), Name::Key);
    assert_eq!(missing, Finding::KeyMissing);
    assert_eq!(Check::new(missing).status(), Status::Warning);

    let dir = ready_home();
    assert_eq!(finding(dir.path(), Name::Key), Finding::KeyPrivate);
    // Rewritten in place, so the file keeps its private access on every platform.
    fs::OpenOptions::new()
        .write(true)
        .open(dir.path().join("identity.key"))
        .and_then(|file| file.set_len(5))
        .expect("truncate");
    let short = finding(dir.path(), Name::Key);
    assert_eq!(short, Finding::KeyWrongSize(5));
    assert_eq!(Check::new(short).failure(), Some(ErrorKind::Storage));
}

#[cfg(unix)]
#[test]
fn a_key_and_a_home_that_others_can_reach_are_two_policy_failures_at_once() {
    use std::os::unix::fs::PermissionsExt;
    let dir = ready_home();
    let key = dir.path().join("identity.key");
    fs::set_permissions(&key, fs::Permissions::from_mode(0o640)).expect("chmod");
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o755)).expect("chmod");
    let loose = finding(dir.path(), Name::Key);
    assert_eq!(loose, Finding::KeyOpenToOthers(0o640));
    assert_eq!(Check::new(loose).failure(), Some(ErrorKind::Policy));
    let open_home = finding(dir.path(), Name::Access);
    assert_eq!(open_home, Finding::AccessOpenToOthers(0o755));
    assert_eq!(Check::new(open_home).failure(), Some(ErrorKind::Policy));
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).expect("chmod");
    assert_eq!(finding(dir.path(), Name::Access), Finding::AccessPrivate);
    assert_eq!(
        finding(dir.path(), Name::Key),
        Finding::KeyOpenToOthers(0o640)
    );
}

#[test]
fn free_space_under_2_gib_is_a_warning_and_the_real_volume_answers() {
    let at = |free| Check::new(Finding::SpaceFree(free)).status();
    assert_eq!(at(0), Status::Warning);
    assert_eq!(at(LOW_SPACE_BYTES - 1), Status::Warning);
    assert_eq!(at(LOW_SPACE_BYTES), Status::Ok);
    assert_eq!(at(u64::MAX), Status::Ok);
    let unreadable = Check::new(Finding::SpaceUnreadable("no volume".into()));
    assert_eq!(
        (unreadable.status(), unreadable.failure()),
        (Status::Warning, None)
    );
    let dir = ready_home();
    assert!(matches!(
        finding(dir.path(), Name::Space),
        Finding::SpaceFree(_)
    ));
}

#[test]
fn a_doctor_run_writes_nothing_whatever_it_finds() {
    let dir = ready_home();
    fs::write(dir.path().join("lock.pid"), "4242\n").expect("write");
    fs::write(dir.path().join("VERSION"), "9\n").expect("write");
    fs::remove_dir(dir.path().join("blobs")).expect("remove");
    let before = contents(dir.path());
    examine_at(dir.path(), false);
    assert_eq!(contents(dir.path()), before);
}

#[test]
fn every_failure_has_a_kind_and_nothing_else_has_one() {
    let location = Location {
        path: "home".into(),
        from_env: false,
    };
    let storage = [
        Finding::HomeMissing(location.clone()),
        Finding::HomeNotAFolder(location.clone()),
        Finding::HomeFoldersMissing(location.clone(), vec!["db"]),
        Finding::HomeUnreadable(location.clone(), "denied".into()),
        Finding::HomeNowhere,
        Finding::LayoutUnreadable("x".into()),
        Finding::LockUnreadable("x".into()),
        Finding::AccessUnreadable("x".into()),
        Finding::KeyWrongSize(31),
        Finding::KeyUnreadable("x".into()),
    ];
    let policy = [
        Finding::LayoutNewer { found: 2, known: 1 },
        Finding::HomeFolderIsLink(location.clone(), "blobs"),
        Finding::HomeFolderOwnedByOther(location.clone(), "blobs"),
        Finding::KeyOpenToOthers(0o644),
        Finding::KeyIsLink,
        Finding::KeyNotAFile,
        Finding::KeyHasLinks(2),
        Finding::KeyOwnedByOther,
        Finding::AccessOpenToOthers(0o755),
        Finding::AccessOwnedByOther,
        Finding::KeyAclNotPrivate,
        Finding::AccessAclNotPrivate,
    ];
    for (findings, kind) in [
        (&storage[..], ErrorKind::Storage),
        (&policy[..], ErrorKind::Policy),
    ] {
        for finding in findings {
            let check = Check::new(finding.clone());
            assert_eq!(
                (check.status(), check.failure()),
                (Status::Failed, Some(kind)),
                "{finding:?}"
            );
        }
    }
    for finding in [
        Finding::HomeReady(location),
        Finding::AccessPrivate,
        Finding::LayoutCurrent(1),
        Finding::LockFree,
        Finding::LockHeld(None),
        Finding::KeyPrivate,
        Finding::SpaceFree(u64::MAX),
    ] {
        assert_eq!(Check::new(finding).status(), Status::Ok);
    }
}
