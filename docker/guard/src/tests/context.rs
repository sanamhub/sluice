//! The build context. BuildKit reads `Dockerfile.dockerignore` beside each Dockerfile, and the
//! legacy builder reads only `docker/.dockerignore`, so the two must let in the same files. Order
//! is part of a rule: a later `!` re-includes what an earlier line excluded, so each image's rules
//! must appear in the shared file in the same order. The Dockerfiles must copy only what the image
//! needs, and only from the guard's folder.

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

/// Whether `needle` appears in `haystack` in the same order, possibly with other lines between.
fn is_subsequence(needle: &[String], haystack: &[String]) -> bool {
    let mut rest = haystack.iter();
    needle.iter().all(|rule| rest.any(|line| line == rule))
}

#[test]
fn the_legacy_builders_ignore_file_is_the_union_of_the_two_images_in_order() {
    let relay = rules(&read(&relay_dir().join("Dockerfile.dockerignore")));
    let rendezvous = rules(&read(&rendezvous_dir().join("Dockerfile.dockerignore")));
    let shared = rules(&read(&docker_dir().join(".dockerignore")));
    assert_eq!(
        shared.first().map(String::as_str),
        Some("*"),
        "an allowlist"
    );
    // Each image's rules, in its own order, or a re-included path could be excluded again.
    for (image, rules) in [("relay", &relay), ("rendezvous", &rendezvous)] {
        assert_eq!(rules.first().map(String::as_str), Some("*"), "{image}");
        assert!(
            is_subsequence(rules, &shared),
            "docker/{image}/Dockerfile.dockerignore is not in the shared file's order"
        );
    }
    // And the shared file lets in nothing that neither image names.
    let both: BTreeSet<&String> = relay.iter().chain(&rendezvous).collect();
    let named: BTreeSet<&String> = shared.iter().collect();
    assert_eq!(both, named);
}

/// The `COPY` and `ADD` instructions of a Dockerfile, in order.
fn copies(dockerfile: &str) -> Vec<String> {
    dockerfile
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with("COPY ") || line.starts_with("ADD "))
        .map(str::to_string)
        .collect()
}

#[test]
fn each_dockerfile_copies_only_what_the_image_needs() {
    let common = [
        "COPY --from=build /out/bin/iroh-relay /usr/local/bin/iroh-relay",
        "COPY --from=build /out/bin/sluice-relay-guard /usr/local/bin/sluice-relay-guard",
    ];
    let guard = [
        "COPY guard/Cargo.toml guard/Cargo.lock /src/guard/",
        "COPY guard/src /src/guard/src",
    ];
    let mut expected = guard.to_vec();
    expected.extend(common);
    expected.push("COPY relay/relay.toml relay/open.toml /etc/relay/");
    assert_eq!(
        copies(&read(&relay_dir().join("Dockerfile"))),
        expected,
        "docker/relay/Dockerfile"
    );

    let mut expected = guard.to_vec();
    expected.extend(common);
    expected.push("COPY rendezvous/manual.toml /etc/rendezvous/");
    assert_eq!(
        copies(&read(&rendezvous_dir().join("Dockerfile"))),
        expected,
        "docker/rendezvous/Dockerfile"
    );
}
