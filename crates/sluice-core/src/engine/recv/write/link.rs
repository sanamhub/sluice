//! Symlinks a user allowed. Every target is checked against the folder as it will be when the tree
//! is complete: a target is resolved one component at a time, and a component that the transfer
//! makes into a link is replaced by that link's target before the next `..` is applied, which is
//! what the operating system does. A link is replaced as an entry and never followed while writing.

use std::{collections::BTreeMap, fs, path::Path};

use super::super::{ReceiveOptions, storage};
use crate::{
    Error,
    manifest::{Kind, Manifest},
};

/// How many links a target may pass through before it is refused, the limit Linux and macOS use.
/// A cycle would otherwise resolve for ever, and a long chain is a way to hide where a link lands.
const MAX_LINKS: usize = 40;

/// Checks every link in the manifest against the tree the transfer is about to make, then writes
/// them. The check is a decision about the manifest alone, so the order the links are written in
/// cannot change an answer.
pub(super) fn write_all(
    manifest: &Manifest,
    options: &ReceiveOptions,
    resume: bool,
) -> Result<(), Error> {
    let links: BTreeMap<&str, &str> = manifest
        .entries()
        .iter()
        .filter(|entry| entry.kind == Kind::Symlink)
        .filter_map(|entry| {
            let target = entry.link_target.as_ref()?;
            Some((entry.path.as_str(), target.as_str()))
        })
        .collect();
    for (path, target) in &links {
        let resolved = resolve_in(&links, path, target).ok_or_else(|| outside(path))?;
        if !stays_inside(&options.out, &resolved)? {
            return Err(outside(path));
        }
    }
    for (path, target) in &links {
        write_one(path, target, options, resume)?;
    }
    Ok(())
}

fn outside(path: &str) -> Error {
    Error::Policy(format!(
        "the link at {path} points outside the output folder, directly or through another link"
    ))
}

/// Writes one symlink entry, or refuses it because its target could leave the output folder.
fn write_one(
    path: &str,
    target: &str,
    options: &ReceiveOptions,
    resume: bool,
) -> Result<(), Error> {
    let link = path
        .split('/')
        .fold(options.out.clone(), |at, name| at.join(name));
    match fs::symlink_metadata(&link) {
        Ok(meta) if meta.is_dir() => {
            return Err(Error::Policy(format!(
                "{} is a folder, so --force cannot replace it with a link",
                link.display()
            )));
        }
        Ok(_) if options.force => {
            fs::remove_file(&link).map_err(|e| storage::policy_path("replace", &link, e))?;
        }
        Ok(meta) if resume && meta.file_type().is_symlink() && read_link_is(&link, target) => {
            return Ok(());
        }
        Ok(_) => return Err(storage::clash(&link)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(storage::policy_path("check", &link, e)),
    }
    create(&link, target)
}

/// The target resolved from the folder that holds the link, one component at a time. A component
/// that names another link of this transfer is replaced by that link's target, as the operating
/// system would resolve it. `None` when a `..` would step above the output folder or a chain is
/// longer than the limit.
fn resolve_in(
    links: &BTreeMap<&str, &str>,
    link: &str,
    target: &str,
) -> Option<std::path::PathBuf> {
    let mut parts: Vec<&str> = match link.rsplit_once('/') {
        Some((parent, _)) => parent.split('/').collect(),
        None => Vec::new(),
    };
    let mut queue: Vec<&str> = target.split('/').rev().collect();
    let mut follows = 0usize;
    while let Some(part) = queue.pop() {
        match part {
            "" | "." => return None,
            ".." => {
                parts.pop()?;
            }
            name => {
                parts.push(name);
                let at = parts.join("/");
                if let Some(next) = links.get(at.as_str()) {
                    follows += 1;
                    if follows > MAX_LINKS {
                        return None;
                    }
                    parts.pop();
                    queue.extend(next.split('/').rev());
                }
            }
        }
    }
    Some(
        parts
            .iter()
            .fold(std::path::PathBuf::new(), |at, part| at.join(part)),
    )
}

/// Whether every existing prefix of `resolved` is inside `out`, with links followed. A prefix that
/// does not exist ends the check: nothing below it can exist either.
fn stays_inside(out: &Path, resolved: &Path) -> Result<bool, Error> {
    let real_out = fs::canonicalize(out).map_err(|e| storage::policy_path("resolve", out, e))?;
    let mut at = out.to_path_buf();
    for component in resolved.components() {
        at.push(component);
        match fs::symlink_metadata(&at) {
            Ok(_) => {
                let real =
                    fs::canonicalize(&at).map_err(|e| storage::policy_path("resolve", &at, e))?;
                if !real.starts_with(&real_out) {
                    return Ok(false);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => break,
            Err(e) => return Err(storage::policy_path("resolve", &at, e)),
        }
    }
    Ok(true)
}

fn read_link_is(link: &Path, target: &str) -> bool {
    fs::read_link(link)
        .ok()
        .is_some_and(|found| found.as_os_str() == target)
}

#[cfg(unix)]
fn create(link: &Path, target: &str) -> Result<(), Error> {
    std::os::unix::fs::symlink(target, link)
        .map_err(|e| storage::policy_path("create a symlink at", link, e))
}

#[cfg(windows)]
fn create(link: &Path, target: &str) -> Result<(), Error> {
    std::os::windows::fs::symlink_file(target, link).map_err(|e| {
        Error::Policy(format!(
            "cannot create a symlink at {}: {e}; Windows needs developer mode or an elevated \
             process for links",
            link.display()
        ))
    })
}

#[cfg(not(any(unix, windows)))]
fn create(_link: &Path, _target: &str) -> Result<(), Error> {
    Err(Error::Policy(
        "this platform has no symlink call".to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::{resolve_in, stays_inside};
    use std::collections::BTreeMap;
    use std::path::Path;

    fn links(pairs: &[(&'static str, &'static str)]) -> BTreeMap<&'static str, &'static str> {
        pairs.iter().copied().collect()
    }

    #[test]
    fn a_target_that_steps_above_the_output_is_refused() {
        let none = BTreeMap::new();
        let link = "sub/link";
        assert!(resolve_in(&none, link, "../x").is_some());
        assert!(resolve_in(&none, link, "a/x").is_some());
        assert!(resolve_in(&none, link, "../../x").is_none());
        assert_eq!(
            resolve_in(&none, link, "../x").unwrap(),
            Path::new("x"),
            "the link's own folder is part of the resolution"
        );
    }

    #[test]
    fn a_link_at_the_top_of_the_output_cannot_step_above_it() {
        let none = BTreeMap::new();
        assert!(resolve_in(&none, "link", "x").is_some());
        assert!(resolve_in(&none, "link", "../x").is_none());
    }

    #[test]
    fn a_target_resolves_through_the_link_the_transfer_makes() {
        // `sub/a -> ..` lands on the output folder, so `sub/z -> a/..` lands above it. Reading
        // `a` as a plain folder would keep `z` inside, which is not where the operating system
        // takes it.
        let first = links(&[("sub/a", ".."), ("sub/z", "a/..")]);
        assert!(resolve_in(&first, "sub/z", "a/..").is_none());
        // The same chain with the links in the other path order, which is the other entry order.
        let second = links(&[("sub/m", "z/.."), ("sub/z", "..")]);
        assert!(resolve_in(&second, "sub/m", "z/..").is_none());
    }

    #[test]
    fn a_chain_of_three_links_that_escapes_is_refused() {
        let chain = links(&[("sub/a", "b/.."), ("sub/b", "c/.."), ("sub/c", "..")]);
        assert!(resolve_in(&chain, "sub/a", "b/..").is_none());
    }

    #[test]
    fn a_chain_that_loops_is_refused() {
        let looped = links(&[("a", "b"), ("b", "a")]);
        assert!(resolve_in(&looped, "a", "b").is_none());
    }

    #[test]
    fn a_folder_that_is_really_a_link_out_of_the_output_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("out");
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(&out).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, out.join("d")).unwrap();
        #[cfg(windows)]
        {
            if std::os::windows::fs::symlink_dir(&outside, out.join("d")).is_err() {
                eprintln!("SKIPPED: this account cannot create a symlink on Windows");
                return;
            }
        }
        // `d/new` does not exist, so a check of the final target alone would see nothing wrong.
        let resolved = out.join("d").join("new").join("x");
        assert!(!stays_inside(&out, &resolved).unwrap());
    }
}
