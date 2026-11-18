//! The build context. BuildKit reads `Dockerfile.dockerignore` beside the Dockerfile, and the
//! legacy builder reads only `docker/.dockerignore`, so the two must let in the same files. Order
//! is part of a rule: a later `!` re-includes what an earlier line excluded, so the shared file
//! must hold the image's rules in the same order. The Dockerfile must copy only what the image
//! needs, and only from the guard's folder.

use super::{read, relay_dir};

/// The lines of an ignore file that are rules: no comment, no blank line.
fn rules(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_string)
        .collect()
}

#[test]
fn the_relay_build_lets_in_the_meeting_place_and_both_workspaces() {
    let image = rules(&read(&relay_dir().join("Dockerfile.dockerignore")));
    assert_eq!(image.first().map(String::as_str), Some("*"), "an allowlist");
    for needed in [
        "!crates/sluice-meet/Cargo.toml",
        "!crates/sluice-meet/src",
        "!docker/guard/Cargo.toml",
        "!docker/guard/src",
        "!docker/relay-server/Cargo.toml",
        "!docker/relay-server/src",
    ] {
        assert!(image.iter().any(|rule| rule == needed), "{needed}");
    }
    assert!(
        !image.iter().any(|rule| rule.contains("target")),
        "{image:?}"
    );
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
    let expected: Vec<String> = [
        "COPY docker/guard/Cargo.toml docker/guard/Cargo.lock /src/docker/guard/",
        "COPY docker/guard/src /src/docker/guard/src",
        "COPY crates/sluice-meet/Cargo.toml /src/crates/sluice-meet/",
        "COPY crates/sluice-meet/words.txt /src/crates/sluice-meet/",
        "COPY crates/sluice-meet/src /src/crates/sluice-meet/src",
        "COPY docker/relay-server/Cargo.toml docker/relay-server/Cargo.lock /src/docker/relay-server/",
        "COPY docker/relay-server/src /src/docker/relay-server/src",
        "COPY --from=build /out/release/sluice-relay /usr/local/bin/sluice-relay",
        "COPY --from=build /out/bin/sluice-relay-guard /usr/local/bin/sluice-relay-guard",
        "COPY docker/relay/relay.toml docker/relay/open.toml docker/relay/meeting-point.toml /etc/relay/",
    ]
    .map(String::from)
    .to_vec();
    assert_eq!(
        context_readers(&read(&relay_dir().join("Dockerfile"))),
        expected,
        "docker/relay/Dockerfile"
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
