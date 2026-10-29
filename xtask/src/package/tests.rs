//! The generator against fixed checksums: the files it writes must be the ones committed under
//! `packaging/expected`, so a template change is a diff a reviewer sees.

use std::collections::BTreeMap;

use super::{TARGETS, files};

/// A checksum per artifact that stands in for a release's, so this runs without a release: the
/// artifact's own bytes repeated to 64 hex characters (32 bytes).
fn sums() -> BTreeMap<String, String> {
    TARGETS
        .iter()
        .map(|target| {
            let artifact = target.artifact();
            let hash: String = artifact
                .bytes()
                .cycle()
                .take(32)
                .map(|byte| format!("{byte:02x}"))
                .collect();
            (artifact, hash)
        })
        .collect()
}

#[test]
fn the_package_files_of_a_release_are_what_the_templates_give() {
    let version = "1.2.3";
    let got = files(version, &sums()).unwrap();
    let want = [
        (
            "homebrew/sluice.rb",
            include_str!("../../../packaging/expected/homebrew/sluice.rb"),
        ),
        (
            "scoop/sluice.json",
            include_str!("../../../packaging/expected/scoop/sluice.json"),
        ),
        (
            "winget/sanamhub.sluice.yaml",
            include_str!("../../../packaging/expected/winget/sanamhub.sluice.yaml"),
        ),
        (
            "winget/sanamhub.sluice.installer.yaml",
            include_str!("../../../packaging/expected/winget/sanamhub.sluice.installer.yaml"),
        ),
        (
            "winget/sanamhub.sluice.locale.en-US.yaml",
            include_str!("../../../packaging/expected/winget/sanamhub.sluice.locale.en-US.yaml"),
        ),
        (
            "aur/PKGBUILD",
            include_str!("../../../packaging/expected/aur/PKGBUILD"),
        ),
    ];
    assert_eq!(got.len(), want.len());
    for ((name, text), (want_name, want_text)) in got.iter().zip(want) {
        assert_eq!(name, want_name);
        assert_eq!(text, want_text, "{name}");
    }
}

/// A version becomes part of a URL and a file name, so anything but a release version is refused.
#[test]
fn a_version_that_is_not_a_version_is_refused() {
    for version in ["v1.2.3", "1.2.3/../x", "", "latest", "-1"] {
        assert!(files(version, &sums()).is_err(), "{version:?}");
    }
    assert!(files("1.2.3", &sums()).is_ok());
    assert!(files("1.2.3-rc.1", &sums()).is_ok());
}

/// A release without a checksum for one target cannot be pinned, so the refusal says which one.
#[test]
fn a_missing_checksum_names_the_artifact() {
    let mut sums = sums();
    let missing = "sluice-cli-aarch64-apple-darwin.tar.xz";
    sums.remove(missing);
    let err = files("1.2.3", &sums).unwrap_err().to_string();
    assert!(err.contains(missing), "{err}");
}
