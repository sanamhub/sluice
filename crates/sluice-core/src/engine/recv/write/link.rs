//! Symlinks a user allowed. A target is checked as text against the output folder, so a target
//! that does not exist yet cannot escape, and against the file system, so a target reached through
//! a link that is already there cannot either. A link is replaced as an entry and never followed.

use std::{
    fs,
    path::{Component, Path},
};

use super::super::{ReceiveOptions, storage};
use crate::{Error, manifest::Entry};

/// Writes one symlink entry, or refuses it because its target could leave the output folder.
pub(super) fn write(entry: &Entry, options: &ReceiveOptions, resume: bool) -> Result<(), Error> {
    let Some(target) = entry.link_target.as_ref().map(|target| target.as_str()) else {
        return Err(Error::Bug("a symlink entry has no target".to_string()));
    };
    let link = options.out.join(entry.path.as_str());
    let Some(resolved) = resolve(&options.out, &link, target) else {
        return Err(Error::Policy(
            "the link points outside the output folder".to_string(),
        ));
    };
    if !stays_inside(&options.out, &resolved)? {
        return Err(Error::Policy(
            "the link points outside the output folder".to_string(),
        ));
    }
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

/// The target resolved from the folder that holds the link, one component at a time. `None` when a
/// `..` would step above the output folder.
fn resolve(out: &Path, link: &Path, target: &str) -> Option<std::path::PathBuf> {
    let relative = link.strip_prefix(out).ok()?;
    let mut parts: Vec<std::ffi::OsString> = Vec::new();
    if let Some(parent) = relative.parent() {
        for component in parent.components() {
            match component {
                Component::Normal(name) => parts.push(name.to_os_string()),
                Component::CurDir => {}
                _ => return None,
            }
        }
    }
    for component in Path::new(target).components() {
        match component {
            Component::Normal(name) => parts.push(name.to_os_string()),
            Component::CurDir => {}
            Component::ParentDir => {
                parts.pop()?;
            }
            Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    Some(
        parts
            .iter()
            .fold(out.to_path_buf(), |at, part| at.join(part)),
    )
}

/// Whether every existing prefix of `resolved` is inside `out`, with links followed. A prefix that
/// does not exist ends the check: nothing below it can exist either.
fn stays_inside(out: &Path, resolved: &Path) -> Result<bool, Error> {
    let real_out = fs::canonicalize(out).map_err(|e| storage::policy_path("resolve", out, e))?;
    let relative = match resolved.strip_prefix(out) {
        Ok(relative) => relative,
        Err(_) => return Ok(false),
    };
    let mut at = out.to_path_buf();
    for component in relative.components() {
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
    use super::{resolve, stays_inside};
    use std::path::Path;

    #[test]
    fn a_target_that_steps_above_the_output_is_refused() {
        let out = Path::new("/out");
        let link = out.join("sub/link");
        assert!(resolve(out, &link, "../x").is_some());
        assert!(resolve(out, &link, "a/x").is_some());
        assert!(resolve(out, &link, "../../x").is_none());
        assert_eq!(
            resolve(out, &link, "../x").unwrap(),
            Path::new("/out/x"),
            "the link's own folder is part of the resolution"
        );
    }

    #[test]
    fn a_link_at_the_top_of_the_output_cannot_step_above_it() {
        let out = Path::new("/out");
        let link = out.join("link");
        assert!(resolve(out, &link, "x").is_some());
        assert!(resolve(out, &link, "../x").is_none());
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
        let resolved = resolve(&out, &out.join("d/new"), "x").unwrap();
        assert!(!stays_inside(&out, &resolved).unwrap());
        assert!(resolve(&out, &out.join("d/new"), "../../inside").is_none());
    }
}
