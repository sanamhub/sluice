//! Which receipt paths are refused: every path that is `--out` or inside it, however it is spelled.

use std::path::{Path, PathBuf};

use super::{key, refuse_under_out};

fn refused(receipt: &Path, out: &Path) -> bool {
    refuse_under_out(receipt, out).is_err()
}

fn made(dir: &tempfile::TempDir, name: &str) -> PathBuf {
    let path = dir.path().join(name);
    std::fs::create_dir(&path).unwrap();
    path
}

/// `--out` itself and anything inside it is refused, whether or not the folder exists yet, and a
/// path beside it is not.
#[test]
fn out_and_everything_inside_it_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let out = made(&dir, "received");
    assert!(refused(&out, &out));
    assert!(refused(&out.join("receipt.txt"), &out));
    assert!(refused(&out.join("sub").join("deep").join("r.txt"), &out));
    assert!(!refused(&dir.path().join("receipt.txt"), &out));
    assert!(!refused(&dir.path().join("received-2").join("r.txt"), &out));

    let later = dir.path().join("later");
    assert!(refused(&later.join("r.txt"), &later));
    assert!(!later.exists(), "the check created the folder");
}

/// The comparison folds what a person can spell differently: `.`, `..` and case.
#[test]
fn paths_are_compared_without_dots_or_case() {
    assert_eq!(
        key(Path::new("inbox/./Sub/../Report.TXT")),
        key(Path::new("INBOX/report.txt"))
    );
    assert_ne!(
        key(Path::new("inbox/report.txt")),
        key(Path::new("inbox/report.txt.bak"))
    );
    let dir = tempfile::tempdir().unwrap();
    let out = made(&dir, "received");
    let spelled = out.join("sub").join("..").join("RECEIPT.txt");
    assert!(refused(&spelled, &out));
}

/// A link to `--out` is `--out`: a receipt written through it lands in the output folder.
#[cfg(unix)]
#[test]
fn a_symlink_to_out_is_out() {
    let dir = tempfile::tempdir().unwrap();
    let out = made(&dir, "received");
    let link = dir.path().join("alias");
    std::os::unix::fs::symlink(&out, &link).unwrap();
    assert!(refused(&link.join("receipt.txt"), &out));
    assert!(refused(&link, &out));
    assert!(refused(&out.join("receipt.txt"), &link));
    let elsewhere = made(&dir, "elsewhere");
    assert!(!refused(&elsewhere.join("receipt.txt"), &link));
}

/// A junction to `--out` is `--out`.
#[cfg(windows)]
#[test]
fn a_junction_to_out_is_out() {
    let dir = tempfile::tempdir().unwrap();
    let out = made(&dir, "received");
    let junction = dir.path().join("alias");
    let made_link = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&junction)
        .arg(&out)
        .output()
        .unwrap();
    assert!(made_link.status.success(), "mklink /J failed");
    assert!(refused(&junction.join("receipt.txt"), &out));
    assert!(refused(&out.join("receipt.txt"), &junction));
    let elsewhere = made(&dir, "elsewhere");
    assert!(!refused(&elsewhere.join("receipt.txt"), &junction));
}

/// The 8.3 short name of a folder is the folder. A volume without short names cannot show it, and
/// the test says so.
#[cfg(windows)]
#[test]
#[allow(unsafe_code)]
fn a_short_name_of_out_is_out() {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};

    use windows_sys::Win32::Storage::FileSystem::GetShortPathNameW;

    let dir = tempfile::tempdir().unwrap();
    let out = made(&dir, "received with a long name");
    let long: Vec<u16> = out.as_os_str().encode_wide().chain([0]).collect();
    let mut short = vec![0u16; 1024];
    // SAFETY: `long` is NUL-terminated and `short` is the length it is told.
    let len = unsafe { GetShortPathNameW(long.as_ptr(), short.as_mut_ptr(), 1024) } as usize;
    let short = PathBuf::from(std::ffi::OsString::from_wide(&short[..len]));
    if len == 0 || key(&short) == key(&out) {
        eprintln!("skipped: this volume has no 8.3 short name for {out:?}");
        return;
    }
    assert!(refused(&short.join("receipt.txt"), &out));
    assert!(refused(&out.join("receipt.txt"), &short));
}

/// A verbatim `\\?\` spelling of a path is the plain path, in either position.
#[cfg(windows)]
#[test]
fn a_verbatim_path_is_the_plain_path() {
    let dir = tempfile::tempdir().unwrap();
    let out = made(&dir, "received");
    let plain = std::path::absolute(out.join("receipt.txt")).unwrap();
    let verbatim = PathBuf::from(format!(r"\\?\{}", plain.display()));
    assert!(refused(&verbatim, &out));
    let verbatim_out = PathBuf::from(format!(
        r"\\?\{}",
        std::path::absolute(&out).unwrap().display()
    ));
    assert!(refused(&plain, &verbatim_out));
    assert!(!refused(&dir.path().join("receipt.txt"), &verbatim_out));
}
