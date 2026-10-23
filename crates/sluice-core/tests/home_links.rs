//! A link planted in a home, by anyone who can write there, must not send the store's own files
//! somewhere else. `lock` and `lock.pid` are opened without following a link and must be plain
//! files, so a link at either is refused and left where it is. The file the link points at,
//! outside the home, is left as it was.

use std::fs;
use std::path::Path;

use sluice_core::{Error, Home};

const PRECIOUS: &str = "not the store's: the lock must never touch this\n";

/// A home folder that already exists, and a file outside it that a planted link points at.
fn home_and_outside() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let outside = dir.path().join("outside.txt");
    fs::create_dir(&home).unwrap();
    fs::write(&outside, PRECIOUS).unwrap();
    (dir, home, outside)
}

/// A link at `link` to the file `target`. A file symlink on Windows needs a right an ordinary user
/// may lack. The test then says so and passes, unless `SLUICE_REQUIRE_SYMLINKS=1`, which the CI
/// legs set because their runners have the right: a skip there would hide a broken check.
fn link_file(target: &Path, link: &Path) -> bool {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link).unwrap();
        true
    }
    #[cfg(windows)]
    {
        match std::os::windows::fs::symlink_file(target, link) {
            Ok(()) => true,
            // ERROR_PRIVILEGE_NOT_HELD, which std has no kind for.
            Err(e) if e.raw_os_error() == Some(1314) => {
                assert!(
                    !std::env::var_os("SLUICE_REQUIRE_SYMLINKS").is_some_and(|v| !v.is_empty()),
                    "SLUICE_REQUIRE_SYMLINKS is set, and this user may not make a symbolic link: {e}"
                );
                eprintln!("SKIPPED: this user may not make a symbolic link: {e}");
                false
            }
            Err(e) => panic!("symlink_file: {e}"),
        }
    }
}

#[test]
fn a_link_planted_at_lock_is_refused_and_what_it_points_at_is_left_alone() {
    let (_dir, home, outside) = home_and_outside();
    if !link_file(&outside, &home.join("lock")) {
        return;
    }
    let Err(Error::Policy(message)) = Home::open(&home) else {
        panic!("a link as the lock was not refused");
    };
    assert!(message.contains("is a link"), "{message}");
    assert!(
        message.contains(&home.join("lock").display().to_string()),
        "{message}"
    );
    assert_eq!(fs::read_to_string(&outside).unwrap(), PRECIOUS);
    assert!(
        fs::symlink_metadata(home.join("lock"))
            .unwrap()
            .file_type()
            .is_symlink(),
        "the link is left as it is"
    );
    assert!(
        !home.join("lock.pid").exists(),
        "nothing was written after the refusal"
    );
}

#[test]
fn a_link_planted_at_lock_pid_is_refused_and_what_it_points_at_is_left_alone() {
    let (_dir, home, outside) = home_and_outside();
    if !link_file(&outside, &home.join("lock.pid")) {
        return;
    }
    let Err(Error::Policy(message)) = Home::open(&home) else {
        panic!("a link as the pid file was not refused");
    };
    assert!(message.contains("is a link"), "{message}");
    assert!(
        message.contains(&home.join("lock.pid").display().to_string()),
        "{message}"
    );
    assert_eq!(fs::read_to_string(&outside).unwrap(), PRECIOUS);
    assert!(
        fs::symlink_metadata(home.join("lock.pid"))
            .unwrap()
            .file_type()
            .is_symlink(),
        "nothing removed or replaced the link"
    );
}

/// A junction is a link a user on Windows may make with no right at all. At either name it points
/// at a folder, which is not a file, so the open is refused as a policy error and what is in the
/// folder stays as it was.
#[cfg(windows)]
#[test]
fn a_junction_planted_at_lock_or_lock_pid_is_refused() {
    for name in ["lock", "lock.pid"] {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let outside = dir.path().join("outside");
        fs::create_dir(&home).unwrap();
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("keep.txt"), PRECIOUS).unwrap();
        let status = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(home.join(name))
            .arg(&outside)
            .stdout(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "mklink /J failed");
        let Err(Error::Policy(message)) = Home::open(&home) else {
            panic!("{name}: a junction was accepted");
        };
        assert!(message.contains("is a link"), "{name}: {message}");
        assert!(
            message.contains(&home.join(name).display().to_string()),
            "{name}: {message}"
        );
        assert!(
            fs::symlink_metadata(home.join(name))
                .unwrap()
                .file_type()
                .is_symlink(),
            "{name}: the junction is left as it is"
        );
        let mut inside: Vec<_> = fs::read_dir(&outside)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        inside.sort();
        assert_eq!(inside, ["keep.txt"], "{name}: nothing was added through it");
        assert_eq!(
            fs::read_to_string(outside.join("keep.txt")).unwrap(),
            PRECIOUS
        );
    }
}
