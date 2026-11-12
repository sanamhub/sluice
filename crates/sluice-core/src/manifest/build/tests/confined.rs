//! The confined walk: what a daemon refuses below a folder it sends, and that a folder swapped for
//! a link while the walk runs cannot lead it out of the folder it was handed.

use std::fs;
use std::path::Path;

use crate::Error;
use crate::engine::open_folder;
use crate::manifest::{Build, ConfinedArg, Manifest};

fn folder_arg(folder: &Path) -> ConfinedArg {
    let name = folder.file_name().unwrap().to_string_lossy().into_owned();
    ConfinedArg::folder(open_folder(folder).unwrap(), name)
}

fn build(folder: &Path) -> Result<Build, Error> {
    Manifest::build_confined(vec![folder_arg(folder)])
}

fn refused(folder: &Path) -> String {
    match build(folder) {
        Err(Error::Policy(text)) => text,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

/// A link to the folder `target`: a symbolic link, or on Windows a junction, which needs no
/// privilege.
pub(super) fn link_to_folder(link: &Path, target: &Path) {
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
    // The one-shot walk does not refuse it, which is what `sluice send` relies on.
    assert!(Manifest::build_paths(&[sent]).is_ok());
}

/// A subfolder swapped for a link to a folder outside, at the exact point between the check of the
/// folder and the listing of it: the listing is relative to the handle and never follows the link,
/// so the files outside are not in the manifest. A walk that listed by path would list them.
#[test]
fn a_subfolder_swapped_for_a_link_before_it_is_listed_leads_nowhere() {
    let dir = tempfile::tempdir().unwrap();
    let sent = dir.path().join("sent");
    let outside = dir.path().join("outside");
    fs::create_dir_all(sent.join("sub")).unwrap();
    fs::create_dir(&outside).unwrap();
    fs::write(sent.join("sub").join("inside.txt"), b"in").unwrap();
    fs::write(outside.join("secret.txt"), b"outside bytes").unwrap();

    let (sub, gone, target) = (sent.join("sub"), sent.join("gone"), outside.clone());
    super::super::below::before_listing(move |listing| {
        if listing.ends_with("/sub") {
            fs::rename(&sub, &gone).unwrap();
            link_to_folder(&sub, &target);
        }
    });
    let result = build(&sent);
    super::super::below::before_listing(|_| {});
    match result {
        Err(Error::Policy(text)) => assert!(text.contains("sent/sub"), "{text}"),
        Ok(Build::Ready(built)) => {
            let names: Vec<&str> = built
                .manifest
                .entries()
                .iter()
                .map(|e| e.path.as_str())
                .collect();
            assert!(!names.iter().any(|n| n.contains("secret")), "{names:?}");
        }
        other => panic!("{other:?}"),
    }
}

/// The ancestor of a file argument swapped for a link after its folder was opened: the file is found
/// below the handle that was proved, so the bytes are the ones inside, not the ones the link leads
/// to.
#[test]
fn an_ancestor_of_a_file_swapped_for_a_link_after_the_open_leads_nowhere() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("root");
    let outside = dir.path().join("outside");
    fs::create_dir_all(root.join("a").join("b")).unwrap();
    fs::create_dir_all(outside.join("b")).unwrap();
    fs::write(root.join("a").join("b").join("data.txt"), b"inside bytes").unwrap();
    fs::write(outside.join("b").join("data.txt"), b"outside bytes").unwrap();
    // The parent folder is opened and proved, then an ancestor is swapped.
    let parent = open_folder(&root.join("a").join("b")).unwrap();
    let moved = fs::rename(root.join("a"), root.join("a-moved"));
    if cfg!(windows) {
        // Windows does not let a folder with an open handle below it be renamed, which is the same
        // protection from the other side: the ancestor cannot be swapped at all.
        assert!(
            moved.is_err(),
            "an ancestor of an open folder cannot be renamed"
        );
        return;
    }
    moved.unwrap();
    link_to_folder(&root.join("a"), &outside);

    let arg = ConfinedArg::file(parent, "data.txt".to_string());
    let Build::Ready(built) = Manifest::build_confined(vec![arg]).unwrap() else {
        panic!("a plain file builds");
    };
    let below = built.disk.below(0).expect("found below its folder");
    let mut body = Vec::new();
    std::io::Read::read_to_end(&mut below.open().unwrap(), &mut body).unwrap();
    assert_eq!(body, b"inside bytes");
}
