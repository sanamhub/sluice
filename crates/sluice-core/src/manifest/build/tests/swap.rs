//! A folder that changes between the walk and the hash. Another local user who can write into it
//! can put a link or a FIFO where the walk saw a regular file, and the hash must not follow the
//! link or wait on the FIFO.

use super::common::*;
use crate::manifest::build::hash::{HashFault, hash_file_with};
use crate::manifest::build::walk::portable;
use crate::manifest::link::LinkTarget;
use crate::manifest::path::ManifestPath;

#[test]
fn hashing_a_path_that_is_now_a_symlink_to_a_readable_file_is_a_change() {
    let dir = tempfile::tempdir().expect("a temporary folder");
    put(dir.path(), "id_ed25519", b"a key this user can read");
    let swapped = dir.path().join("report.txt");
    if !symlink("id_ed25519", &swapped) {
        return;
    }
    let result = hash_file_with(&swapped, || {});
    assert!(matches!(result, Err(HashFault::Changed)), "{result:?}");
}

#[test]
fn a_windows_target_has_its_separators_made_slashes() {
    let link = |path: &str| ManifestPath::parse(path).expect("a path");
    let up = portable("..\\b", true);
    assert_eq!(up, "../b");
    let up = LinkTarget::parse_rule(&up).expect("passes rules 1 to 4");
    assert!(up.stays_inside(&link("a/link")));

    let through = portable("a\\..\\b", true);
    assert_eq!(through, "a/../b");
    let through = LinkTarget::parse_rule(&through).expect("passes rules 1 to 4");
    assert!(through.stays_inside(&link("link")));

    // A drive path keeps its colon, which rule 4 refuses through path rule 6.
    let drive = portable("C:\\x", true);
    assert_eq!(drive, "C:/x");
    assert_eq!(LinkTarget::parse_rule(&drive).err(), Some(4));
}

#[test]
fn a_unix_target_keeps_its_backslashes_for_rule_4_to_refuse() {
    for text in ["..\\b", "a\\..\\b", "C:\\x"] {
        let kept = portable(text, false);
        assert_eq!(kept, text);
        assert_eq!(LinkTarget::parse_rule(&kept).err(), Some(4), "{text:?}");
    }
}

#[cfg(unix)]
mod unix {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::mpsc;
    use std::time::Duration;

    use super::*;
    use crate::manifest::Reason;

    #[test]
    fn hashing_a_path_that_is_now_a_fifo_is_a_change_at_once() {
        let dir = tempfile::tempdir().expect("a temporary folder");
        let pipe = dir.path().join("pipe");
        let made = std::process::Command::new("mkfifo").arg(&pipe).status();
        if !made.is_ok_and(|status| status.success()) {
            eprintln!("SKIPPED: mkfifo is not available");
            return;
        }
        // A plain open of a FIFO waits for a writer. If that comes back, the thread hangs and
        // the timeout fails the test instead of the whole run hanging.
        let (sent, got) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = sent.send(hash_file_with(&pipe, || {}));
        });
        let result = got
            .recv_timeout(Duration::from_secs(10))
            .expect("the hash returned without waiting for a writer");
        assert!(matches!(result, Err(HashFault::Changed)), "{result:?}");
    }

    #[test]
    fn a_folder_to_send_that_cannot_be_listed_is_listed_as_a_dot() {
        let dir = tempfile::tempdir().expect("a temporary folder");
        put(dir.path(), "inner.txt", b"x");
        let set = |mode: u32| {
            fs::set_permissions(dir.path(), fs::Permissions::from_mode(mode)).expect("chmod");
        };
        set(0o000);
        if fs::read_dir(dir.path()).is_ok() {
            set(0o755);
            eprintln!("SKIPPED: this account lists folders whatever their mode (root)");
            return;
        }
        let refusal = refused(dir.path());
        set(0o755);
        assert_eq!(refusal.problems.len(), 1);
        assert_eq!(refusal.problems[0].path, ".");
        assert!(matches!(refusal.problems[0].reason, Reason::Unreadable(_)));
    }
}
