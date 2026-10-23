//! The build context. BuildKit reads `Dockerfile.dockerignore` beside each Dockerfile, and the
//! legacy builder reads only `docker/.dockerignore`, so the two must let in the same files, and the
//! Dockerfiles must copy only the guard's manifest, lock file and sources from the guard's folder.

use std::collections::BTreeSet;

use super::{docker_dir, read, relay_dir, rendezvous_dir};

/// The lines of an ignore file that are rules: no comment, no blank line.
fn rules(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_string)
        .collect()
}

#[test]
fn the_legacy_builders_ignore_file_is_the_union_of_the_two_images() {
    let both: BTreeSet<String> = [relay_dir(), rendezvous_dir()]
        .iter()
        .flat_map(|dir| rules(&read(&dir.join("Dockerfile.dockerignore"))))
        .collect();
    let shared = rules(&read(&docker_dir().join(".dockerignore")));
    assert_eq!(
        shared.first().map(String::as_str),
        Some("*"),
        "an allowlist"
    );
    assert_eq!(shared.iter().cloned().collect::<BTreeSet<_>>(), both);
}

#[test]
fn each_dockerfile_copies_only_the_guards_manifest_lock_file_and_sources() {
    for dir in [relay_dir(), rendezvous_dir()] {
        let dockerfile = read(&dir.join("Dockerfile"));
        let copies: Vec<&str> = dockerfile
            .lines()
            .filter(|line| line.starts_with("COPY guard"))
            .collect();
        assert_eq!(
            copies,
            [
                "COPY guard/Cargo.toml guard/Cargo.lock /src/guard/",
                "COPY guard/src /src/guard/src"
            ],
            "{}",
            dir.display()
        );
    }
}
