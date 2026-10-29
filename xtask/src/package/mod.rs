//! The package files a release needs for Homebrew, Scoop, winget and the AUR, written from the
//! checksums the release itself carries. The templates live here rather than in a folder of their
//! own, so the tree gains no top-level place for four short formats.

mod checksums;
mod render;

use std::{collections::BTreeMap, path::Path};

use anyhow::Result;

/// The description every package carries, which is the first line `README.md` says about Sluice.
pub const DESCRIPTION: &str =
    "Sluice sends a file straight from one computer to another. Nothing is stored in between.";

/// The project's home, which every package names.
pub const HOMEPAGE: &str = "https://github.com/sanamhub/sluice";

/// The licence, as the release names it.
pub const LICENSE: &str = "MIT OR Apache-2.0";

/// The description's first sentence, for a field with a shorter limit.
pub const SHORT: &str = "Sluice sends a file straight from one computer to another.";

/// The publisher's name in the registries, which is the repository's owner.
pub const PUBLISHER: &str = "sanamhub";

/// One target cargo-dist builds: its triple and the archive kind of its artifact.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Target {
    pub triple: &'static str,
    /// `true` for a `.zip` and `false` for a `.tar.xz`.
    pub zipped: bool,
}

/// Every target a release has, in the order the files name them.
pub const TARGETS: [Target; 6] = [
    Target {
        triple: "aarch64-apple-darwin",
        zipped: false,
    },
    Target {
        triple: "x86_64-apple-darwin",
        zipped: false,
    },
    Target {
        triple: "aarch64-unknown-linux-gnu",
        zipped: false,
    },
    Target {
        triple: "x86_64-unknown-linux-gnu",
        zipped: false,
    },
    Target {
        triple: "aarch64-pc-windows-msvc",
        zipped: true,
    },
    Target {
        triple: "x86_64-pc-windows-msvc",
        zipped: true,
    },
];

impl Target {
    /// The folder the target's archive unpacks to: `dist` names it for the archive, without the
    /// suffix, and puts the binary inside it. Every format that points inside an archive has to
    /// agree with this one spelling, or it points at nothing.
    pub fn folder(&self) -> String {
        format!("sluice-cli-{}", self.triple)
    }

    /// The artifact's file name in the release.
    pub fn artifact(&self) -> String {
        let suffix = if self.zipped { "zip" } else { "tar.xz" };
        format!("{}.{suffix}", self.folder())
    }
}

/// The address of `artifact` in the release of `version`.
pub fn url(version: &str, artifact: &str) -> String {
    format!("{HOMEPAGE}/releases/download/v{version}/{artifact}")
}

/// The checksum the release gives each artifact, by artifact name.
pub type Checksums = BTreeMap<String, String>;

/// The files a release needs, as path and contents. A relative path names the folder each registry
/// takes its file in.
pub fn files(version: &str, sums: &Checksums) -> Result<Vec<(String, String)>> {
    check_version(version)?;
    for target in TARGETS {
        if !sums.contains_key(&target.artifact()) {
            anyhow::bail!(
                "the release has no checksum for {}; pass the folder that holds its `.sha256` files",
                target.artifact()
            );
        }
    }
    Ok(vec![
        (
            "homebrew/sluice.rb".to_string(),
            render::formula(version, sums)?,
        ),
        (
            "scoop/sluice.json".to_string(),
            render::scoop(version, sums)?,
        ),
        (
            "winget/sanamhub.sluice.yaml".to_string(),
            render::winget_version(version),
        ),
        (
            "winget/sanamhub.sluice.installer.yaml".to_string(),
            render::winget_installer(version, sums)?,
        ),
        (
            "winget/sanamhub.sluice.locale.en-US.yaml".to_string(),
            render::winget_locale(version),
        ),
        ("aur/PKGBUILD".to_string(), render::pkgbuild(version, sums)?),
    ])
}

/// Writes the package files of `version` into `out`, reading the release's checksums from `from`, a
/// folder of the `.sha256` files the release carries.
pub fn run(version: &str, from: &Path, out: &Path) -> Result<()> {
    let sums = checksums::read(from)?;
    std::fs::create_dir_all(out)
        .map_err(|e| anyhow::anyhow!("cannot make {}: {e}", out.display()))?;
    for (name, text) in files(version, &sums)? {
        let path = out.join(&name);
        if let Some(folder) = path.parent() {
            std::fs::create_dir_all(folder)
                .map_err(|e| anyhow::anyhow!("cannot make {}: {e}", folder.display()))?;
        }
        std::fs::write(&path, text)
            .map_err(|e| anyhow::anyhow!("cannot write {}: {e}", path.display()))?;
        println!("{}", path.display());
    }
    Ok(())
}

/// A version is a release tag without its `v`: digits and dots, with an optional pre-release after
/// a hyphen. Anything else would be pasted into a URL and a file name.
fn check_version(version: &str) -> Result<()> {
    let well_formed = !version.is_empty()
        && version
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+'))
        && version.starts_with(|c: char| c.is_ascii_digit());
    if !well_formed {
        anyhow::bail!("{version:?} is not a release version: give it without the leading v");
    }
    Ok(())
}

#[cfg(test)]
mod tests;
