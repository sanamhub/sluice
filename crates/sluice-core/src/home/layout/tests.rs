use std::fs;
use std::path::Path;

use super::{Step, parse_version, read_version, refuse_newer_than, settle_with};
use crate::Error;

fn version_of(dir: &Path) -> String {
    fs::read_to_string(dir.join("VERSION")).unwrap()
}

fn step_one(home: &Path) -> Result<(), Error> {
    fs::write(home.join("ran-1-to-2"), "").unwrap();
    Ok(())
}

fn step_two(home: &Path) -> Result<(), Error> {
    fs::write(home.join("ran-2-to-3"), "").unwrap();
    Ok(())
}

fn failing_step(_: &Path) -> Result<(), Error> {
    Err(Error::Storage("the step failed".into()))
}

const STEPS: &[Step] = &[step_one, step_two];

#[test]
fn a_home_with_no_version_gets_the_latest() {
    let dir = tempfile::tempdir().unwrap();
    settle_with(dir.path(), &[]).unwrap();
    assert_eq!(version_of(dir.path()), "1\n");
    let dir = tempfile::tempdir().unwrap();
    settle_with(dir.path(), STEPS).unwrap();
    assert_eq!(version_of(dir.path()), "3\n");
    assert!(
        !dir.path().join("ran-1-to-2").exists(),
        "a new home has nothing to migrate"
    );
}

#[test]
fn a_lower_version_runs_through_every_step_after_it_and_no_others() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("VERSION"), "1\n").unwrap();
    settle_with(dir.path(), STEPS).unwrap();
    assert_eq!(version_of(dir.path()), "3\n");
    assert!(dir.path().join("ran-1-to-2").exists() && dir.path().join("ran-2-to-3").exists());

    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("VERSION"), "2\n").unwrap();
    settle_with(dir.path(), STEPS).unwrap();
    assert_eq!(version_of(dir.path()), "3\n");
    assert!(!dir.path().join("ran-1-to-2").exists() && dir.path().join("ran-2-to-3").exists());
}

#[test]
fn the_current_version_runs_nothing_and_a_newer_one_is_refused_untouched() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("VERSION"), "3\n").unwrap();
    settle_with(dir.path(), STEPS).unwrap();
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);

    fs::write(dir.path().join("VERSION"), "4\n").unwrap();
    let Err(Error::Policy(message)) = settle_with(dir.path(), STEPS) else {
        panic!("expected a policy error");
    };
    assert!(
        message.contains("version 4") && message.contains("version 3"),
        "{message}"
    );
    assert_eq!(version_of(dir.path()), "4\n");
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn a_step_that_fails_stops_the_run_and_keeps_the_last_version_that_finished() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("VERSION"), "1\n").unwrap();
    let steps: &[Step] = &[step_one, failing_step, step_two];
    assert!(settle_with(dir.path(), steps).is_err());
    assert_eq!(version_of(dir.path()), "2\n");
    assert!(dir.path().join("ran-1-to-2").exists() && !dir.path().join("ran-2-to-3").exists());
}

#[test]
fn the_message_for_a_newer_layout_names_both_versions() {
    let Err(Error::Policy(message)) = refuse_newer_than(7, 1) else {
        panic!("expected a policy error");
    };
    assert!(
        message.contains("version 7") && message.contains("version 1"),
        "{message}"
    );
    assert!(refuse_newer_than(1, 1).is_ok());
}

#[test]
fn only_a_plain_decimal_number_from_1_is_a_version() {
    let path = Path::new("VERSION");
    for good in [
        ("1", 1),
        ("1\n", 1),
        ("1\r\n", 1),
        ("42\n", 42),
        ("999999999", 999_999_999),
    ] {
        assert_eq!(parse_version(good.0, path).unwrap(), good.1, "{:?}", good.0);
    }
    for bad in [
        "",
        "\n",
        "0\n",
        "-1",
        "+1",
        " 1",
        "1 ",
        "1.0",
        "abc",
        "1\n2",
        "1000000000",
        "99999999999",
        "\u{661}",
    ] {
        assert!(
            matches!(parse_version(bad, path), Err(Error::Storage(_))),
            "{bad:?}"
        );
    }
}

#[test]
fn a_missing_version_file_reads_as_none() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(read_version(dir.path()).unwrap(), None);
    fs::write(dir.path().join("VERSION"), "1\n").unwrap();
    assert_eq!(read_version(dir.path()).unwrap(), Some(1));
}

#[cfg(unix)]
mod fresh;
