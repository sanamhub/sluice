//! Pipes: a ticket on standard input, a raw blob from standard input, and content written to
//! standard output instead of a file.

mod common;

use std::{
    io::Write,
    process::{Command, Stdio},
};

use common::{EXE, data, start, start_stdin, wait_for_sender};

/// The store for a write to standard output lives under the home's private `tmp`, and it is gone
/// when the command ends. The message says whether the lock of a folder that was left can still be
/// taken: a folder nobody holds is one no command is removing from.
fn assert_no_temporary_store(home: &std::path::Path) {
    let mut left = Vec::new();
    for entry in std::fs::read_dir(home.join("tmp"))
        .into_iter()
        .flatten()
        .flatten()
    {
        let path = entry.path();
        let state = match std::fs::File::open(path.join("lock")) {
            Ok(file) if file.try_lock().is_ok() => "free",
            Ok(_) => "held",
            Err(_) => "no lock",
        };
        left.push(format!("{} ({state})", entry.file_name().to_string_lossy()));
    }
    assert!(
        left.is_empty(),
        "a temporary store was left under the home: {left:?}"
    );
}

#[test]
fn a_ticket_on_standard_input_is_read() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let landing = dir.path().join("landing");
    std::fs::create_dir(&landing).unwrap();
    let file = dir.path().join("piped.bin");
    let body = data(4096, 9);
    std::fs::write(&file, &body).unwrap();

    let (mut sender, ticket) = start(&home, &file, "1h");
    let mut child = Command::new(EXE)
        .env("SLUICE_HOME", &home)
        .current_dir(&landing)
        .args(["recv", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start recv");
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(format!("{ticket}\n").as_bytes())
        .unwrap();
    let result = child.wait_with_output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(std::fs::read(landing.join("piped.bin")).unwrap(), body);
    wait_for_sender(&mut sender);
}

#[test]
fn standard_input_crosses_as_a_raw_blob() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let body = data(200 * 1024, 5);
    let (mut sender, ticket) = start_stdin(&home, &body, "1h");

    let result = Command::new(EXE)
        .env("SLUICE_HOME", &home)
        .args(["recv", &ticket, "--out", "-"])
        .output()
        .expect("run recv");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(result.stdout, body);
    assert!(
        String::from_utf8_lossy(&result.stderr).contains("wrote 204800 bytes"),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    // The sender shares this home and keeps its store in it until it ends, so the home is only
    // finished being written when both commands are done.
    wait_for_sender(&mut sender);
    assert_no_temporary_store(&home);
}

#[test]
fn an_empty_pipe_is_a_valid_raw_blob() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let (mut sender, ticket) = start_stdin(&home, b"", "1h");
    let result = Command::new(EXE)
        .env("SLUICE_HOME", &home)
        .args(["recv", &ticket, "--out", "-"])
        .output()
        .expect("run recv");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(result.stdout.is_empty());
    wait_for_sender(&mut sender);
    assert_no_temporary_store(&home);
}

#[test]
fn a_one_file_transfer_goes_to_standard_output() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let file = dir.path().join("one.bin");
    let body = data(150 * 1024, 8);
    std::fs::write(&file, &body).unwrap();

    let (mut sender, ticket) = start(&home, &file, "1h");
    let result = Command::new(EXE)
        .env("SLUICE_HOME", &home)
        .args(["recv", &ticket, "--out", "-"])
        .output()
        .expect("run recv");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(result.stdout, body);
    wait_for_sender(&mut sender);
    assert_no_temporary_store(&home);
}

/// A file another program holds under the store's folder for a moment does not keep the folder
/// behind: the command waits for the removal it started, tries it again before it ends, and the
/// home it leaves is the one it found.
#[cfg(windows)]
#[test]
fn a_store_another_program_holds_for_a_moment_is_still_removed() {
    use std::{
        os::windows::fs::OpenOptionsExt,
        path::{Path, PathBuf},
        time::{Duration, Instant},
    };

    const FILE_LIST_DIRECTORY: u32 = 0x0001;
    const FILE_READ_ATTRIBUTES: u32 = 0x0080;
    const FILE_SHARE_READ: u32 = 0x0000_0001;
    const FILE_SHARE_WRITE: u32 = 0x0000_0002;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;

    /// The store folder the receive makes under the home's `tmp`, waited for because its name is
    /// random.
    fn wait_for_folder(home: &Path) -> PathBuf {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            for entry in std::fs::read_dir(home.join("tmp"))
                .into_iter()
                .flatten()
                .flatten()
            {
                let path = entry.path();
                if path.join("store").is_dir() {
                    return path.join("store");
                }
            }
            assert!(Instant::now() < deadline, "the receive made no store");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let file = dir.path().join("held.bin");
    let body = data(256 * 1024, 3);
    std::fs::write(&file, &body).unwrap();
    let (mut sender, ticket) = start(&home, &file, "1h");

    let child = Command::new(EXE)
        .env("SLUICE_HOME", &home)
        .args(["recv", &ticket, "--out", "-"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start recv");
    // A handle that withholds delete sharing is what a scanner holds a folder with. It outlives
    // the removal's own waits, so the folder is only gone if the command tries again before it ends.
    let scanning = {
        let home = home.clone();
        std::thread::spawn(move || {
            let store = wait_for_folder(&home);
            let held = std::fs::OpenOptions::new()
                .access_mode(FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES)
                .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
                .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
                .open(&store)
                .expect("hold the store folder");
            // Held for longer than the waits one removal takes, so only a command that tries again
            // before it ends leaves the home without the folder.
            std::thread::sleep(Duration::from_millis(2200));
            drop(held);
        })
    };
    let result = child.wait_with_output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(result.stdout, body);
    scanning.join().unwrap();
    wait_for_sender(&mut sender);
    assert_no_temporary_store(&home);
}

#[test]
fn a_folder_is_refused_on_standard_output() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let tree = dir.path().join("tree");
    std::fs::create_dir(&tree).unwrap();
    std::fs::write(tree.join("a.txt"), b"one").unwrap();
    std::fs::write(tree.join("b.txt"), b"two").unwrap();

    let (sender, ticket) = start(&home, &tree, "1h");
    let result = Command::new(EXE)
        .env("SLUICE_HOME", &home)
        .args(["recv", &ticket, "--out", "-"])
        .output()
        .expect("run recv");
    assert_eq!(result.status.code(), Some(6));
    assert!(result.stdout.is_empty());
    let text = String::from_utf8_lossy(&result.stderr);
    assert!(
        text.contains("folder cannot be written to standard output"),
        "{text}"
    );
    drop(sender);
}

#[test]
fn json_cannot_share_standard_output_with_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let file = dir.path().join("json.bin");
    std::fs::write(&file, b"bytes").unwrap();
    let (_sender, ticket) = start(&home, &file, "1h");
    let result = Command::new(EXE)
        .env("SLUICE_HOME", &home)
        .args(["--json", "recv", &ticket, "--out", "-"])
        .output()
        .expect("run recv");
    assert_eq!(result.status.code(), Some(6));
    assert!(result.stdout.is_empty());
    let text = String::from_utf8_lossy(&result.stderr);
    assert!(text.contains("--out - cannot share it"), "{text}");
}
