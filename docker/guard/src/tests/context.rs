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

/// A Dockerfile instruction as Docker reads it: the keyword, and the rest after the space or tab
/// that has to follow it. A line whose keyword is not followed by whitespace, a comment and a blank
/// line are not instructions. The keyword is returned as written: Docker matches it without regard
/// to case, so `copy` is a `COPY`.
fn instruction(line: &str) -> Option<(&str, &str)> {
    let line = line.trim_start();
    let end = line.find([' ', '\t']).filter(|end| *end > 0)?;
    let (keyword, rest) = line.split_at(end);
    if !keyword.bytes().all(|byte| byte.is_ascii_alphabetic()) {
        return None;
    }
    Some((keyword, rest.trim_start()))
}

/// Whether a `RUN`'s arguments mount the build context. A `type=bind` mount without a source
/// mounts the context root, and `source=.` or `from=.` names it directly. A cache mount does not.
fn mount_names_context(run: &str) -> bool {
    let mut words = run.split_whitespace();
    while let Some(word) = words.next() {
        let options = if let Some(value) = word.strip_prefix("--mount=") {
            value
        } else if word == "--mount" {
            match words.next() {
                Some(value) => value,
                None => return false,
            }
        } else {
            continue;
        };
        if options.split(',').any(|option| {
            let option = option.trim();
            option.eq_ignore_ascii_case("type=bind")
                || option.starts_with("source=.")
                || option.starts_with("from=.")
        }) {
            return true;
        }
    }
    false
}

/// The lines of a Dockerfile that read the build context: every `COPY` and `ADD`, and a `RUN` that
/// mounts the context. Anything else, such as a cache mount, reads no file of it.
fn context_readers(dockerfile: &str) -> Vec<String> {
    dockerfile
        .lines()
        .filter(|line| {
            let Some((keyword, rest)) = instruction(line) else {
                return false;
            };
            keyword.eq_ignore_ascii_case("copy")
                || keyword.eq_ignore_ascii_case("add")
                || (keyword.eq_ignore_ascii_case("run") && mount_names_context(rest))
        })
        .map(|line| line.trim_start().to_string())
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
        context_readers(&read(&relay_dir().join("Dockerfile"))),
        expected,
        "docker/relay/Dockerfile"
    );

    let mut expected = guard.to_vec();
    expected.extend(common);
    expected.push("COPY rendezvous/manual.toml /etc/rendezvous/");
    assert_eq!(
        context_readers(&read(&rendezvous_dir().join("Dockerfile"))),
        expected,
        "docker/rendezvous/Dockerfile"
    );
}

/// Docker reads a keyword in any case, and a tab is as good as a space after it, so each of these
/// reads the build context. None of them is one of the lines the image needs, so the test above
/// refuses it.
#[test]
fn a_copy_or_a_mount_in_another_case_is_refused() {
    let expected = "COPY guard/src /src/guard/src";
    for line in [
        "copy guard/src /src/guard/src",
        "Add\tguard/src /src/guard/src",
        "COPY\tguard/src /src/guard/src",
    ] {
        assert_eq!(context_readers(line), [line], "{line}");
        assert_ne!(
            context_readers(line),
            [expected],
            "{line} has to differ from the line the image needs"
        );
    }
    for line in [
        "RUN --mount=type=bind,target=/src cargo build",
        "RUN --mount=type=cache,target=/cargo --mount=type=bind cargo build",
        "run --mount=type=bind,target=/src cargo build",
    ] {
        assert_eq!(context_readers(line), [line], "{line}");
    }
}

/// A cache mount of the context reads nothing of it, and a `COPY` in a comment or a word that only
/// starts like an instruction is not one.
#[test]
fn a_line_that_reads_no_file_of_the_context_is_not_a_copy() {
    for line in [
        "RUN --mount=type=cache,target=/cargo cargo build",
        "# COPY . /src",
        "COPYING nonsense",
        "COPY",
        " FROM alpine",
    ] {
        assert!(context_readers(line).is_empty(), "{line}");
    }
}
