//! The link check: targets resolved the way the operating system resolves them, and the one key a
//! name is looked up by.

use std::collections::BTreeMap;
use std::path::Path;

use super::{resolve, stays_inside};
use crate::manifest::path::fold_key;

fn links(pairs: &[(&'static str, &'static str)]) -> BTreeMap<String, (&'static str, &'static str)> {
    pairs
        .iter()
        .map(|&(path, target)| (fold_key(path), (path, target)))
        .collect()
}

/// An output folder that holds nothing on disk, so a check sees only the manifest's links.
fn empty_out() -> &'static Path {
    Path::new("a-folder-that-does-not-exist")
}

#[test]
fn a_target_that_steps_above_the_output_is_refused() {
    let none = BTreeMap::new();
    let link = "sub/link";
    assert!(resolve(&none, empty_out(), link, "../x").is_ok());
    assert!(resolve(&none, empty_out(), link, "a/x").is_ok());
    assert!(resolve(&none, empty_out(), link, "../../x").is_err());
    assert_eq!(
        resolve(&none, empty_out(), link, "../x").unwrap(),
        Path::new("x"),
        "the link's own folder is part of the resolution"
    );
}

#[test]
fn a_link_at_the_top_of_the_output_cannot_step_above_it() {
    let none = BTreeMap::new();
    assert!(resolve(&none, empty_out(), "link", "x").is_ok());
    assert!(resolve(&none, empty_out(), "link", "../x").is_err());
}

#[test]
fn a_target_resolves_through_the_link_the_transfer_makes() {
    // `sub/a -> ..` lands on the output folder, so `sub/z -> a/..` lands above it. Reading
    // `a` as a plain folder would keep `z` inside, which is not where the operating system
    // takes it.
    let first = links(&[("sub/a", ".."), ("sub/z", "a/..")]);
    assert!(resolve(&first, empty_out(), "sub/z", "a/..").is_err());
    // The same chain with the links in the other path order, which is the other entry order.
    let second = links(&[("sub/m", "z/.."), ("sub/z", "..")]);
    assert!(resolve(&second, empty_out(), "sub/m", "z/..").is_err());
}

/// A target that names one of the transfer's links in another spelling is refused: the check
/// cannot know whether this volume reads the two as one name, and following the link would be
/// wrong where it does not. `ß` and `ss` are one name to full case folding and two to both NTFS
/// and Linux, which is the case that made the guess unsafe.
#[test]
fn a_target_that_spells_a_links_name_differently_is_refused() {
    let fold = links(&[("sub/ß", "a/b/c"), ("sub/z", "ss/../../../..")]);
    let err = resolve(&fold, empty_out(), "sub/z", "ss/../../../..").unwrap_err();
    assert!(matches!(err, crate::Error::Policy(_)), "{err}");
    let text = err.to_string();
    assert!(text.contains("sub/ß"), "{text}");
    assert!(text.contains("sub/ss"), "{text}");

    // Case alone is the same question, and so is a pair one volume folds and another does not.
    let ascii = links(&[("sub/A", ".."), ("sub/z", "a/../..")]);
    assert!(resolve(&ascii, empty_out(), "sub/z", "a/../..").is_err());
    let accent = links(&[("sub/É", ".."), ("sub/z", "é/../..")]);
    assert!(resolve(&accent, empty_out(), "sub/z", "é/../..").is_err());
}

/// The same question on a disk that already holds the other spelling: the output has a real
/// `sub/ss` folder, and the manifest makes `sub/ß` a link out of the tree. Reading the folder as
/// the link follows the link, while the operating system resolves the real folder and lands
/// above the output, so the transfer is refused and both names are in the message.
#[test]
fn a_real_folder_whose_name_folds_to_a_links_name_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out");
    std::fs::create_dir_all(out.join("sub").join("ss")).unwrap();
    let fold = links(&[("sub/ß", "a/b/c"), ("sub/z", "ss/../../../..")]);
    let err = resolve(&fold, &out, "sub/z", "ss/../../../..").unwrap_err();
    let text = err.to_string();
    assert!(text.contains("sub/ß") && text.contains("sub/ss"), "{text}");
    // The same two names in the other order: the folder is spelled as the link is.
    let other = links(&[("sub/ss", "a/b/c"), ("sub/z", "ß/../../../..")]);
    assert!(resolve(&other, &out, "sub/z", "ß/../../../..").is_err());
}

#[test]
fn a_chain_of_three_links_that_escapes_is_refused() {
    let chain = links(&[("sub/a", "b/.."), ("sub/b", "c/.."), ("sub/c", "..")]);
    assert!(resolve(&chain, empty_out(), "sub/a", "b/..").is_err());
}

/// Windows writes a link target with `\` and resolves both separators, so a check that split
/// only on `/` would read `..\x` as a single name and keep a target that goes a folder up.
#[cfg(windows)]
#[test]
fn a_target_written_with_backslashes_is_split_on_them() {
    let none = BTreeMap::new();
    assert_eq!(
        resolve(&none, empty_out(), "sub/link", r"..\x").unwrap(),
        Path::new("x")
    );
    assert!(resolve(&none, empty_out(), "link", r"..\x").is_err());
}
#[test]
fn a_chain_that_loops_is_refused() {
    let looped = links(&[("a", "b"), ("b", "a")]);
    assert!(resolve(&looped, empty_out(), "a", "b").is_err());
}

/// A link that is already in the output is resolved the way the operating system resolves it,
/// before the next `..` is applied. A target that goes through such a link to somewhere else
/// would otherwise land beside the output: `foo/../x` reads as `x` only if `foo` is a folder.
#[cfg(unix)]
#[test]
fn a_target_through_a_link_already_in_the_output_is_resolved_through_it() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out");
    let outside = dir.path().join("outside");
    std::fs::create_dir_all(&out).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, out.join("foo")).unwrap();
    assert!(
        resolve(&BTreeMap::new(), &out, "l", "foo/../x").is_err(),
        "a target through a link out of the output was followed to the wrong place"
    );

    // The same spelling with a link that stays inside lands inside.
    std::fs::create_dir_all(out.join("bar")).unwrap();
    std::os::unix::fs::symlink("bar", out.join("baz")).unwrap();
    assert_eq!(
        resolve(&BTreeMap::new(), &out, "l", "baz/../x").unwrap(),
        Path::new("x")
    );
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
/// The one key is what a target is looked up by, so a target that names one of the transfer's links
/// in another spelling of that key is refused with both names in the message. The dotless i is read
/// as `i` and a default-ignorable code point is read away, so `I` names a link made at `ı`, and `a`
/// names one made at `a` with a zero-width joiner in it.
#[test]
fn a_target_in_another_spelling_of_the_key_is_refused() {
    let dotless = links(&[("sub/ı", ".."), ("sub/z", "I/../..")]);
    let err = resolve(&dotless, empty_out(), "sub/z", "I/../..").unwrap_err();
    assert!(matches!(err, crate::Error::Policy(_)), "{err}");
    let text = err.to_string();
    assert!(text.contains("sub/ı") && text.contains("sub/I"), "{text}");

    // The same, with a zero-width character inside the name the link was made at.
    let joined = links(&[("sub/ı\u{200d}", ".."), ("sub/z", "I/../..")]);
    let err = resolve(&joined, empty_out(), "sub/z", "I/../..").unwrap_err();
    let text = err.to_string();
    assert!(
        text.contains("sub/ı\u{200d}") && text.contains("sub/I"),
        "{text}"
    );

    // And with the key reading away the joiner on both sides: `a` names the link at `a` + U+200D.
    let zero = links(&[("sub/a\u{200d}", ".."), ("sub/z", "a/../..")]);
    let err = resolve(&zero, empty_out(), "sub/z", "a/../..").unwrap_err();
    let text = err.to_string();
    assert!(
        text.contains("sub/a\u{200d}") && text.contains("sub/a"),
        "{text}"
    );
}

/// Two spellings that are canonically equivalent are one name, because the key ends with NFKC. A
/// fold can leave a composition undone, so without that step `Ϊ́` and `ΐ` would be two names here
/// and one on a volume that composes them, and the target would step above the output.
#[test]
fn a_canonically_equivalent_spelling_of_a_links_name_is_refused() {
    let accent = links(&[("sub/\u{3aa}\u{301}", ".."), ("sub/z", "\u{390}/../../x")]);
    let err = resolve(&accent, empty_out(), "sub/z", "\u{390}/../../x").unwrap_err();
    assert!(matches!(err, crate::Error::Policy(_)), "{err}");
    let text = err.to_string();
    assert!(
        text.contains("sub/\u{3aa}\u{301}") && text.contains("sub/\u{390}"),
        "{text}"
    );
}

/// The same spellings written the same way are one name and not a question: a target that is exactly
/// the link's path follows it, whatever the key does with it.
#[test]
fn a_target_in_the_same_spelling_follows_the_link() {
    let same = links(&[("sub/ı", "x"), ("sub/z", "ı/..")]);
    assert!(resolve(&same, empty_out(), "sub/z", "ı/..").is_ok());
}
