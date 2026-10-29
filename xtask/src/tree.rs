//! A guard against a build folder in the tree.
//!
//! A build that was given a target path in another form once wrote a folder into the checkout, under
//! a name no tool looks at, and it was committed. The files a build writes into a folder of its own
//! are the ones this looks for, and so is a top-level entry that is not one of the repository's. The
//! list below is the deliberate part: a new top-level entry is added to it in the commit that adds
//! the entry, and anything else at the top is a folder that does not belong there.

use std::{collections::BTreeSet, path::Path, process::Command};

/// The files a build writes into the folder it uses as its own: Cargo writes the first two at the
/// root of a target folder, and a folder that holds build output usually carries a `CACHEDIR.TAG` so
/// that a backup tool skips it. None of them is ever a source file.
const BUILD_FILES: [&str; 3] = ["CACHEDIR.TAG", ".rustc_info.json", ".cargo-lock"];

/// Every top-level entry this repository has.
const TOP_LEVEL: [&str; 31] = [
    ".cargo",
    ".config",
    ".dockerignore",
    ".editorconfig",
    ".gitattributes",
    ".github",
    ".gitignore",
    "CHANGELOG.md",
    "CODE_OF_CONDUCT.md",
    "CONTRIBUTING.md",
    "Cargo.lock",
    "Cargo.toml",
    "LICENSE-APACHE",
    "LICENSE-MIT",
    "README.md",
    "SECURITY.md",
    "SPEC.md",
    "assets",
    "bench",
    "crates",
    "deny.toml",
    "dist-workspace.toml",
    "docker",
    "docs",
    "packaging",
    "release-plz.toml",
    "rust-toolchain.toml",
    "rustfmt.toml",
    "site",
    "tests",
    "xtask",
];

/// How many reasons an error lists before it counts the rest.
const SHOWN: usize = 20;

/// Checks every path git tracks in this repository and says what does not belong.
pub fn run() -> anyhow::Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or_else(|| anyhow::anyhow!("the xtask manifest is in the repository's root"))?;
    let tracked = tracked(root)?;
    let found = problems(&tracked);
    if !found.is_empty() {
        let mut text = found
            .iter()
            .take(SHOWN)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n  ");
        if found.len() > SHOWN {
            text.push_str(&format!("\n  and {} more", found.len() - SHOWN));
        }
        anyhow::bail!("the tree holds paths that do not belong:\n  {text}");
    }
    println!("{} tracked paths, none of them a build's", tracked.len());
    Ok(())
}

/// The reasons these paths do not belong in the tree, and none when they all do. A path is written
/// with `/` between its components, as git writes it.
///
/// A whole folder that does not belong is named once, and the files a build writes under it are not
/// named again: a target folder that was committed holds thousands of paths, and one line about the
/// folder is what a reader needs.
fn problems(paths: &[String]) -> Vec<String> {
    let mut found = Vec::new();
    let mut unlisted = BTreeSet::new();
    for path in paths {
        if let Some(top) = path.split('/').next()
            && !TOP_LEVEL.contains(&top)
        {
            unlisted.insert(top);
        }
    }
    found.extend(
        unlisted
            .iter()
            .map(|top| format!("{top} is not a top-level entry of this repository")),
    );
    for path in paths {
        let top = path.split('/').next().unwrap_or(path);
        let name = path.rsplit('/').next().unwrap_or(path);
        if BUILD_FILES.contains(&name) && !unlisted.contains(top) {
            found.push(format!("{path} is a file a build writes"));
        }
    }
    found
}

/// The paths git tracks, relative to `root`. The index is the list that matters: a file that is only
/// in the working tree is not part of the tree yet, and untracked build output is expected.
fn tracked(root: &Path) -> anyhow::Result<Vec<String>> {
    let output = Command::new("git")
        .current_dir(root)
        .args(["ls-files", "--cached", "-z"])
        .output()
        .map_err(|error| anyhow::anyhow!("cannot run git ls-files: {error}"))?;
    if !output.status.success() {
        anyhow::bail!(
            "git ls-files failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8(output.stdout)
        .map_err(|error| anyhow::anyhow!("git listed a path that is not UTF-8: {error}"))?
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(str::to_string)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::problems;

    fn paths(list: &[&str]) -> Vec<String> {
        list.iter().map(|path| (*path).to_string()).collect()
    }

    /// The tree as it is: every path is under an entry the list names, and none is a build's.
    #[test]
    fn the_repositorys_own_paths_are_all_where_they_belong() {
        assert_eq!(
            problems(&paths(&[
                "README.md",
                ".github/workflows/ci.yml",
                "crates/sluice-core/src/lib.rs",
                "docs/adr/0001-reuse-iroh-stack.md",
                "tests/vectors/paths/rule-1.json",
            ])),
            Vec::<String>::new()
        );
    }

    /// A build folder that was committed, under any name, and in any folder: the name of the file a
    /// build writes is what is wrong, not where it is. A folder that does not belong at the top is
    /// named once, so a target folder with thousands of paths in it is one line and not thousands.
    #[test]
    fn a_file_a_build_writes_is_named_wherever_it_is() {
        for path in [
            "crates/sluice-core/target/.rustc_info.json",
            "site/dist/.rustc_info.json",
            "crates/sluice-core/CACHEDIR.TAG",
        ] {
            let found = problems(&paths(&[path]));
            assert_eq!(found.len(), 1, "{path}: {found:?}");
            assert!(found[0].contains(path), "{found:?}");
        }
        let found = problems(&paths(&[
            "target/CACHEDIR.TAG",
            "target/.rustc_info.json",
            "target/debug/libsluice.rlib",
        ]));
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].contains("target"), "{found:?}");
    }

    /// A folder at the top that the list does not name, which is how a target folder once appeared
    /// in the checkout under a name no tool looks for.
    #[test]
    fn a_top_level_entry_that_is_not_listed_is_named() {
        let found = problems(&paths(&[
            "README.md",
            "C\u{ff1a}Users/me/.cache/sluice-target/debug/libsluice.rlib",
        ]));
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].contains("not a top-level entry"), "{found:?}");
    }

    /// A new top-level entry is one thing said once, however many paths are under it.
    #[test]
    fn one_unlisted_entry_is_reported_once() {
        let found = problems(&paths(&["stray/one", "stray/two", "stray/deep/three"]));
        assert_eq!(found.len(), 1, "{found:?}");
    }
}
