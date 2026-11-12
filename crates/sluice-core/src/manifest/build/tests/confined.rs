//! The confined walk: what a daemon refuses below a folder it sends.

use std::fs;
use std::path::Path;

use crate::Error;
use crate::manifest::{Build, Manifest};

fn build(folder: &Path) -> Result<Build, Error> {
    Manifest::build_paths_confined(&[folder.to_path_buf()])
}

fn refused(folder: &Path) -> String {
    match build(folder) {
        Err(Error::Policy(text)) => text,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

/// A link to the folder `target`: a symbolic link, or on Windows a junction, which needs no
/// privilege.
fn link_to_folder(link: &Path, target: &Path) {
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, link).unwrap();
    #[cfg(windows)]
    {
        let made = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .output()
            .unwrap();
        assert!(made.status.success(), "mklink failed");
    }
}

#[test]
fn a_plain_folder_still_builds() {
    let dir = tempfile::tempdir().unwrap();
    let sent = dir.path().join("sent");
    fs::create_dir_all(sent.join("sub")).unwrap();
    fs::write(sent.join("a.txt"), b"a").unwrap();
    fs::write(sent.join("sub").join("b.txt"), b"b").unwrap();
    let Build::Ready(built) = build(&sent).unwrap() else {
        panic!("a plain folder is built");
    };
    assert_eq!(built.manifest.entries().len(), 4);
}

#[test]
fn a_file_with_a_second_name_anywhere_below_is_refused_by_its_path() {
    let dir = tempfile::tempdir().unwrap();
    let sent = dir.path().join("sent");
    fs::create_dir_all(sent.join("sub")).unwrap();
    let elsewhere = dir.path().join("elsewhere.txt");
    fs::write(&elsewhere, b"secret").unwrap();
    fs::hard_link(&elsewhere, sent.join("sub").join("linked.txt")).unwrap();
    let text = refused(&sent);
    assert!(text.contains("sent/sub/linked.txt"), "{text}");
    assert!(text.contains("more than one name"), "{text}");
}

#[test]
fn a_link_below_the_folder_is_refused_and_a_plain_walk_still_records_it() {
    let dir = tempfile::tempdir().unwrap();
    let sent = dir.path().join("sent");
    let outside = dir.path().join("outside");
    fs::create_dir_all(&sent).unwrap();
    fs::create_dir(&outside).unwrap();
    link_to_folder(&sent.join("door"), &outside);
    let text = refused(&sent);
    assert!(text.contains("sent/door"), "{text}");
    assert!(text.contains("link"), "{text}");
    // The one-shot walk does not refuse it, which is what `sluice send` relies on.
    assert!(Manifest::build_paths(&[sent]).is_ok());
}
