//! The schema is iroh-relay 1.4.0's. The image must build that release, and the probe must resolve
//! it, or a new release could add a key the guard does not know and the guard would refuse a good
//! config, or worse, let a key through that it reads differently.

use super::{read, relay_dir, repo_root};
use crate::IROH_RELAY_VERSION;

/// The `version` of each `[[package]]` named `name` in a Cargo.lock.
fn locked(lock: &str, name: &str) -> Vec<String> {
    lock.split("[[package]]")
        .filter(|entry| {
            entry
                .lines()
                .any(|line| line == format!("name = \"{name}\""))
        })
        .filter_map(|entry| {
            entry
                .lines()
                .find_map(|line| line.strip_prefix("version = "))
        })
        .map(|version| version.trim_matches('"').to_string())
        .collect()
}

#[test]
fn the_dockerfile_builds_the_release_the_schema_was_written_for() {
    let dockerfile = read(&relay_dir().join("Dockerfile"));
    let args: Vec<&str> = dockerfile
        .lines()
        .filter_map(|line| line.strip_prefix("ARG IROH_RELAY_VERSION="))
        .collect();
    assert_eq!(args, [IROH_RELAY_VERSION], "docker/relay/Dockerfile");
}

#[test]
fn the_root_lock_resolves_the_release_the_schema_was_written_for() {
    let lock = read(&repo_root().join("Cargo.lock"));
    assert_eq!(
        locked(&lock, "iroh-relay"),
        [IROH_RELAY_VERSION],
        "Cargo.lock"
    );
}

#[test]
fn every_crate_the_guard_locks_is_one_the_root_lock_has_at_the_same_version() {
    let root = read(&repo_root().join("Cargo.lock"));
    let guard = read(&relay_dir().join("guard/Cargo.lock"));
    let mut names = 0;
    for entry in guard.split("[[package]]").skip(1) {
        let field = |key: &str| {
            entry
                .lines()
                .find_map(|line| line.strip_prefix(key))
                .map(|v| v.trim_matches('"'))
        };
        let (name, version) = (field("name = ").unwrap(), field("version = ").unwrap());
        if name == "sluice-relay-guard" {
            continue;
        }
        assert!(
            locked(&root, name).iter().any(|v| v == version),
            "{name} {version}"
        );
        names += 1;
    }
    assert!(names > 0, "the guard's Cargo.lock lists no dependency");
}
