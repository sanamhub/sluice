//! Opening a home, its lock and its identity key, through the public API and real processes.

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, Stdio};

use sluice_core::{Error, Home};

/// Names the home that the child process of the lock tests holds. Only the parent sets it.
const HOLD_ENV: &str = "SLUICE_TEST_HOLD_HOME";

/// Puts `bytes` in the key file of `dir`, private as a key is made, which `identity` insists on.
fn put_key(dir: &Path, bytes: &[u8]) {
    let key = dir.join("identity.key");
    fs::write(&key, bytes).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&key, fs::Permissions::from_mode(0o600)).unwrap();
    }
}

fn names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn open_creates_the_folders_and_the_version_and_nothing_else() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("new").join("home");
    let home = Home::open(&path).unwrap();
    assert_eq!(home.path(), path);
    assert_eq!(
        names(&path),
        ["VERSION", "blobs", "db", "lock", "lock.pid", "logs"]
    );
    assert_eq!(fs::read_to_string(path.join("VERSION")).unwrap(), "1\n");
    // The identity is made when asked for, not when the home is opened.
    assert!(!path.join("identity.key").exists());
}

#[cfg(unix)]
#[test]
fn on_unix_the_folders_are_private() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("home");
    let _home = Home::open(&path).unwrap();
    for folder in ["", "blobs", "db", "logs"] {
        let mode = fs::metadata(path.join(folder))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o700, "{folder}");
    }
}

#[test]
fn reopening_a_home_changes_nothing_in_it() {
    let dir = tempfile::tempdir().unwrap();
    drop(Home::open(dir.path()).unwrap());
    let before = names(dir.path());
    drop(Home::open(dir.path()).unwrap());
    assert_eq!(names(dir.path()), before);
    assert_eq!(
        fs::read_to_string(dir.path().join("VERSION")).unwrap(),
        "1\n"
    );
}

#[test]
fn a_newer_layout_is_refused_with_both_versions_and_nothing_is_written() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("VERSION"), "2\n").unwrap();
    let Err(Error::Policy(message)) = Home::open(dir.path()) else {
        panic!("expected a policy error");
    };
    assert!(
        message.contains("version 2") && message.contains("version 1"),
        "{message}"
    );
    assert_eq!(names(dir.path()), ["VERSION"]);
    assert_eq!(
        fs::read_to_string(dir.path().join("VERSION")).unwrap(),
        "2\n"
    );
}

#[test]
fn a_version_file_that_holds_no_version_is_a_storage_error_and_is_left_alone() {
    for text in [
        "",
        "abc\n",
        "-1\n",
        "+1\n",
        "0\n",
        "1.0\n",
        "99999999999\n",
        " 1\n",
    ] {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("VERSION"), text).unwrap();
        assert!(
            matches!(Home::open(dir.path()), Err(Error::Storage(_))),
            "{text:?}"
        );
        assert_eq!(
            fs::read_to_string(dir.path().join("VERSION")).unwrap(),
            text
        );
    }
}

#[test]
fn a_version_file_with_a_windows_line_end_is_read() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("VERSION"), "1\r\n").unwrap();
    assert!(Home::open(dir.path()).is_ok());
}

#[test]
fn a_path_that_cannot_be_a_folder_is_a_storage_error_that_names_it() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a-file");
    fs::write(&file, "x").unwrap();
    let Err(Error::Storage(message)) = Home::open(&file) else {
        panic!("expected a storage error");
    };
    assert!(message.contains("a-file"), "{message}");
}

#[test]
fn a_second_open_in_the_same_process_is_refused_until_the_first_is_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let first = Home::open(dir.path()).unwrap();
    let Err(Error::Policy(message)) = Home::open(dir.path()) else {
        panic!("expected a policy error");
    };
    assert!(message.contains("another sluice process"), "{message}");
    // The refused open did not touch the pid of the holder.
    let recorded = fs::read_to_string(dir.path().join("lock.pid")).unwrap();
    assert_eq!(recorded.trim(), std::process::id().to_string());
    drop(first);
    assert!(Home::open(dir.path()).is_ok());
}

#[test]
fn a_lock_file_that_nobody_holds_is_taken_whatever_it_says() {
    let dir = tempfile::tempdir().unwrap();
    drop(Home::open(dir.path()).unwrap());
    fs::write(dir.path().join("lock"), "not a lock, and not a process\n").unwrap();
    fs::write(dir.path().join("lock.pid"), "99999999\n").unwrap();
    assert!(Home::open(dir.path()).is_ok());
    let pid = fs::read_to_string(dir.path().join("lock.pid")).unwrap();
    assert_eq!(pid.trim(), std::process::id().to_string());
}

/// The child of the lock tests. It is this same test binary, started again with `HOLD_ENV` set:
/// it opens the home, says so, and waits to be killed. Without the variable it does nothing,
/// which is what it does in an ordinary run.
#[test]
fn child_that_holds_a_home_until_it_is_killed() {
    let Some(path) = std::env::var_os(HOLD_ENV).map(PathBuf::from) else {
        return;
    };
    let _home = Home::open(&path).expect("the child opens the home");
    println!("locked");
    loop {
        std::thread::park();
    }
}

/// The child process of a lock test, killed when this drops, so a failed assertion does not leave
/// it running with the lock held.
struct Holder {
    child: Child,
    // Kept so the pipe to the child stays open for as long as the test runs.
    _out: BufReader<ChildStdout>,
}

impl Drop for Holder {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Starts the child on `path` and waits until it holds the lock.
fn start_holder(path: &Path) -> Holder {
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "child_that_holds_a_home_until_it_is_killed",
            "--nocapture",
        ])
        .env(HOLD_ENV, path)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let out = BufReader::new(child.stdout.take().unwrap());
    // From here the guard owns the child, so every exit path below kills it.
    let mut holder = Holder { child, _out: out };
    let mut line = String::new();
    while line.trim() != "locked" {
        line.clear();
        assert!(
            holder._out.read_line(&mut line).unwrap() > 0,
            "the child ended before it took the lock"
        );
    }
    holder
}

#[test]
fn another_process_holding_the_lock_is_named_and_a_killed_one_leaves_it_free() {
    let dir = tempfile::tempdir().unwrap();
    let mut holder = start_holder(dir.path());
    let child = &mut holder.child;

    let Err(Error::Policy(message)) = Home::open(dir.path()) else {
        panic!("expected a policy error while the child holds the home");
    };
    assert!(message.contains("another sluice process"), "{message}");
    assert!(
        message.contains(&format!("pid {}", child.id())),
        "{message}"
    );

    // Killed, not asked to stop: it gets no chance to unlock or to clean up.
    child.kill().unwrap();
    child.wait().unwrap();
    let home = Home::open(dir.path()).expect("a dead process holds no lock");
    let recorded = fs::read_to_string(dir.path().join("lock.pid")).unwrap();
    assert_eq!(recorded.trim(), std::process::id().to_string());
    drop(home);
}

#[test]
fn the_identity_is_made_on_first_use_and_is_the_same_afterwards() {
    let dir = tempfile::tempdir().unwrap();
    let first = {
        let home = Home::open(dir.path()).unwrap();
        let id = home.identity().unwrap();
        assert_eq!(home.identity().unwrap().endpoint_id(), id.endpoint_id());
        id.endpoint_id()
    };
    let key = fs::read(dir.path().join("identity.key")).unwrap();
    assert_eq!(key.len(), 32);
    let home = Home::open(dir.path()).unwrap();
    assert_eq!(home.identity().unwrap().endpoint_id(), first);
    assert_eq!(fs::read(dir.path().join("identity.key")).unwrap(), key);
    assert!(
        !names(dir.path()).iter().any(|n| n.ends_with(".tmp")),
        "{:?}",
        names(dir.path())
    );
}

#[cfg(unix)]
#[test]
fn on_unix_the_key_is_mode_0600() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let home = Home::open(dir.path()).unwrap();
    home.identity().unwrap();
    let mode = fs::metadata(dir.path().join("identity.key"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
    home.rotate_identity().unwrap();
    let mode = fs::metadata(dir.path().join("identity.key"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
}

#[test]
fn the_endpoint_id_is_the_public_key_of_the_seed_in_the_file() {
    // The first test vector of RFC 8032.
    let seed = "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60";
    let public = "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a";
    let bytes: Vec<u8> = (0..32)
        .map(|i| u8::from_str_radix(&seed[2 * i..2 * i + 2], 16).unwrap())
        .collect();
    let dir = tempfile::tempdir().unwrap();
    put_key(dir.path(), &bytes);
    let home = Home::open(dir.path()).unwrap();
    let identity = home.identity().unwrap();
    assert_eq!(identity.endpoint_id_hex(), public);
    assert_eq!(
        identity.endpoint_id().to_vec(),
        (0..32)
            .map(|i| u8::from_str_radix(&public[2 * i..2 * i + 2], 16).unwrap())
            .collect::<Vec<u8>>()
    );
    // The file was read, not rewritten.
    assert_eq!(fs::read(dir.path().join("identity.key")).unwrap(), bytes);
}

#[test]
fn a_key_file_of_the_wrong_length_is_an_error_and_is_left_alone() {
    for len in [0usize, 1, 31, 33, 64, 4096] {
        let dir = tempfile::tempdir().unwrap();
        let content = vec![7u8; len];
        put_key(dir.path(), &content);
        let home = Home::open(dir.path()).unwrap();
        let Err(Error::Storage(message)) = home.identity() else {
            panic!("expected a storage error for {len} bytes");
        };
        assert!(
            message.contains("identity.key") && message.contains("32"),
            "{message}"
        );
        assert_eq!(
            fs::read(dir.path().join("identity.key")).unwrap(),
            content,
            "{len}"
        );
    }
}

#[test]
fn an_error_about_a_key_never_holds_the_key() {
    let dir = tempfile::tempdir().unwrap();
    let content = b"SECRETSECRETSECRETSECRETSECRETSECRETSECRET".to_vec();
    put_key(dir.path(), &content);
    let home = Home::open(dir.path()).unwrap();
    let message = home.identity().unwrap_err().to_string();
    assert!(!message.contains("SECRET"), "{message}");
}

#[test]
fn debug_of_an_identity_shows_the_endpoint_id_and_none_of_the_secret() {
    let dir = tempfile::tempdir().unwrap();
    let home = Home::open(dir.path()).unwrap();
    let identity = home.identity().unwrap();
    let secret = fs::read(dir.path().join("identity.key")).unwrap();
    let shown = format!("{identity:?} {identity:#?}");
    assert!(shown.contains(&identity.endpoint_id_hex()));
    let secret_hex: String = secret.iter().map(|b| format!("{b:02x}")).collect();
    let secret_list = format!("{:?}", secret);
    assert!(!shown.contains(&secret_hex), "{shown}");
    assert!(!shown.contains(&secret_list), "{shown}");
    assert!(
        !shown.contains("SigningKey") && !shown.contains("secret"),
        "{shown}"
    );
}

#[test]
fn rotating_replaces_the_key_and_a_later_open_sees_the_new_one() {
    let dir = tempfile::tempdir().unwrap();
    let (old, new) = {
        let home = Home::open(dir.path()).unwrap();
        let old = home.identity().unwrap().endpoint_id();
        let new = home.rotate_identity().unwrap().endpoint_id();
        (old, new)
    };
    assert_ne!(old, new);
    assert_eq!(
        Home::open(dir.path())
            .unwrap()
            .identity()
            .unwrap()
            .endpoint_id(),
        new
    );
    assert!(!names(dir.path()).iter().any(|n| n.ends_with(".tmp")));
}

#[test]
fn rotating_a_home_that_has_no_key_yet_makes_one() {
    let dir = tempfile::tempdir().unwrap();
    let home = Home::open(dir.path()).unwrap();
    let id = home.rotate_identity().unwrap().endpoint_id();
    assert_eq!(home.identity().unwrap().endpoint_id(), id);
}

#[test]
fn an_unfinished_temporary_key_from_a_crash_is_not_a_key_and_is_replaced() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("identity.key.tmp"), b"half").unwrap();
    let home = Home::open(dir.path()).unwrap();
    home.identity().unwrap();
    assert_eq!(fs::read(dir.path().join("identity.key")).unwrap().len(), 32);
    assert!(!dir.path().join("identity.key.tmp").exists());
}
