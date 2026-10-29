//! `cargo xtask man <dir>`: writes the man pages of `sluice`.
//!
//! The pages are made from the clap definition that the binary uses, so they cannot drift from it.
//! They are built when wanted and not checked in or carried inside the binary.

use std::fs;
use std::path::Path;

use anyhow::Context;

/// Writes `sluice.1` and one page per subcommand, such as `sluice-id.1`, into `dir`, which is
/// created if it is missing.
pub fn run(dir: &Path) -> anyhow::Result<()> {
    fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    clap_mangen::generate_to(sluice_cli::command(), dir)
        .with_context(|| format!("cannot write the man pages into {}", dir.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    #[test]
    fn writes_a_page_for_sluice_and_one_for_each_subcommand() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("man");
        super::run(&target).unwrap();
        let mut names: Vec<String> = fs::read_dir(&target)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        assert_eq!(
            names,
            [
                "sluice-completions.1",
                "sluice-doctor.1",
                "sluice-id.1",
                "sluice-inspect.1",
                "sluice-recv.1",
                "sluice-send.1",
                "sluice.1"
            ]
        );
        for name in &names {
            let page = fs::read_to_string(target.join(name)).unwrap();
            assert!(page.contains(".TH"), "{name} is not a man page");
        }
        let main = fs::read_to_string(target.join("sluice.1")).unwrap();
        assert!(
            // roff escapes the hyphen.
            main.contains("sluice\\-id(1)"),
            "the main page links its subcommands"
        );
        // The flags come from the clap definition, so a page that does not name one is a page made
        // from a definition that lost it.
        for name in ["sluice-recv.1", "sluice-inspect.1"] {
            let page = fs::read_to_string(target.join(name)).unwrap();
            assert!(
                page.contains("\\-\\-include"),
                "{name} does not name --include"
            );
        }
    }
}
