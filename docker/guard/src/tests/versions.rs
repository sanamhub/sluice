//! The schema is iroh-relay 1.4.1's. Both images must build that release, and the probe must
//! resolve it, or a new release could add a key the guard does not know and the guard would
//! refuse a good config, or worse, let a key through that it reads differently.

use super::{docker_dir, read, relay_dir, rendezvous_dir, repo_root};
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

/// The one `cargo install` of iroh-relay in each Dockerfile, which must take its version from the
/// argument and not name one of its own.
const INSTALL: &str = "RUN cargo install iroh-relay --version \"=${IROH_RELAY_VERSION}\" --locked \
                       --features server --root /out";

#[test]
fn both_dockerfiles_build_the_release_the_schema_was_written_for() {
    for (image, dir) in [("relay", relay_dir()), ("rendezvous", rendezvous_dir())] {
        let dockerfile = read(&dir.join("Dockerfile"));
        let args: Vec<&str> = dockerfile
            .lines()
            .filter_map(|line| line.strip_prefix("ARG IROH_RELAY_VERSION="))
            .collect();
        assert_eq!(args, [IROH_RELAY_VERSION], "docker/{image}/Dockerfile");
        let installs: Vec<&str> = dockerfile
            .lines()
            .filter(|line| line.contains("cargo install") && line.contains("iroh-relay"))
            .collect();
        assert_eq!(installs, [INSTALL], "docker/{image}/Dockerfile");
    }
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
    let guard = read(&docker_dir().join("guard/Cargo.lock"));
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

/// The `major.minor` of a locked version, `1.1` for `1.1.6+spec-1.1.0`.
fn minor(version: &str) -> String {
    version.split('.').take(2).collect::<Vec<_>>().join(".")
}

/// The image builds iroh-relay from the Cargo.lock it was published with, and the guard parses a
/// config with its own toml and serde. If their major or minor versions differ, the two could read
/// one file by different rules, which is what the guard exists to rule out. The published lock's
/// entries are checked in, with the file they came from, so this runs without the network.
#[test]
fn the_toml_and_serde_iroh_relay_builds_with_are_the_guards_to_the_minor_version() {
    let published = read(&docker_dir().join("guard/testdata/iroh-relay-1.4.1/Cargo.lock.excerpt"));
    let guard = read(&docker_dir().join("guard/Cargo.lock"));
    for name in [
        "toml",
        "toml_parser",
        "toml_datetime",
        "serde",
        "serde_core",
    ] {
        let (theirs, ours) = (locked(&published, name), locked(&guard, name));
        assert_eq!(theirs.len(), 1, "{name} in the published lock");
        assert_eq!(ours.len(), 1, "{name} in the guard's lock");
        assert_eq!(
            minor(&theirs[0]),
            minor(&ours[0]),
            "{name}: {theirs:?} against {ours:?}"
        );
    }
}
