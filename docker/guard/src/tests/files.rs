use std::fs;
use std::io::Read;

#[cfg(target_os = "linux")]
use crate::file::open_regular;
use crate::open_config;

fn scratch() -> std::path::PathBuf {
    // Per test and per process, so parallel runs never share one.
    let dir = std::env::temp_dir().join(format!(
        "sluice-relay-guard-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn refusal(path: &std::path::Path) -> String {
    open_config(path).unwrap_err().to_string()
}

#[test]
fn a_path_that_does_not_exist_is_refused_because_iroh_would_run_open() {
    let dir = scratch();
    let message = refusal(&dir.join("relay.toml"));
    assert!(message.contains("does not exist"), "{message}");
    assert!(
        message.contains("iroh-relay would run open with no limit"),
        "{message}"
    );
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_folder_is_refused() {
    let dir = scratch();
    let message = refusal(&dir);
    assert!(message.contains("is not a regular file"), "{message}");
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_regular_file_is_read_and_handed_back_at_its_start() {
    let dir = scratch();
    let path = dir.join("relay.toml");
    fs::write(&path, "[access]\nallowlist = []\n").unwrap();
    let (mut file, text) = open_config(&path).unwrap();
    assert_eq!(text, "[access]\nallowlist = []\n");
    let mut again = String::new();
    file.read_to_string(&mut again).unwrap();
    assert_eq!(again, text);
    drop(file);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_file_that_is_not_utf8_is_refused() {
    let dir = scratch();
    let path = dir.join("relay.toml");
    fs::write(&path, b"[access]\nallowlist = [\"\xff\"]\n").unwrap();
    let message = refusal(&path);
    assert!(message.contains("is not UTF-8 text"), "{message}");
    fs::remove_dir_all(dir).unwrap();
}

/// The value of `--config-path` is printed only when it looks like a path and cannot be a token.
/// Everything else becomes "the config file given to --config-path", so a token pasted after the
/// flag does not end up in the container log.
#[test]
fn a_config_path_that_may_be_a_token_is_not_printed() {
    let unnamed = "the config file given to --config-path";
    for secret in [
        "s3cr3t-T0KEN",
        "C:\\s3cr3t-T0KEN",
        "/etc/relay.toml?token=s3cr3t",
        "/etc/relay.toml#s3cr3t",
        "/etc has a space/relay.toml",
        "relay.toml",
        "/etc/relay.toml\ns3cr3t",
    ] {
        let message = refusal(std::path::Path::new(secret));
        assert!(!message.contains("s3cr3t"), "{secret:?}: {message}");
        assert!(message.contains(unnamed), "{secret:?}: {message}");
    }
    // A path that looks like one is printed, so an operator can see what was opened.
    let path = "/etc/relay/relay.toml";
    let message = refusal(std::path::Path::new(path));
    assert!(
        message.contains(&format!("the config file {path}")),
        "{message}"
    );
    // The length counts characters, and 200 of them is still a path.
    let longest = format!("/{}", "a".repeat(199));
    assert!(refusal(std::path::Path::new(&longest)).contains(&longest));
    let too_long = format!("/{}", "a".repeat(200));
    assert!(refusal(std::path::Path::new(&too_long)).contains(unnamed));
}

/// Both forms read as a sentence after the verb, so a message never says "the config file the path
/// given to --config-path".
#[test]
fn a_config_file_is_named_with_or_without_its_path() {
    let named = refusal(std::path::Path::new("/etc/relay/x.toml"));
    assert!(
        named.contains("the config file /etc/relay/x.toml does not exist"),
        "{named}"
    );
    let unnamed = refusal(std::path::Path::new("s3cr3t-T0KEN"));
    assert!(
        unnamed.contains("the config file given to --config-path does not exist"),
        "{unnamed}"
    );
    assert!(
        !unnamed.contains("config file the path given"),
        "the two forms are not stacked: {unnamed}"
    );
}

/// A rename over the path after the check, as a ConfigMap update does, changes nothing the relay
/// reads. Opening a `/proc/self/fd` link opens the file the descriptor holds, so the descriptor
/// stands in for the relay's descriptor 0 here.
#[cfg(target_os = "linux")]
#[test]
fn a_rename_over_the_path_after_the_check_does_not_reach_the_relay() {
    use std::os::fd::AsRawFd;

    let dir = scratch();
    let path = dir.join("relay.toml");
    fs::write(&path, "checked\n").unwrap();
    let (file, text) = open_config(&path).unwrap();
    let swapped = dir.join("swapped.toml");
    fs::write(&swapped, "not checked\n").unwrap();
    fs::rename(&swapped, &path).unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "not checked\n");
    let reopened = format!("/proc/self/fd/{}", file.as_raw_fd());
    assert_eq!(fs::read_to_string(reopened).unwrap(), text);
    drop(file);
    fs::remove_dir_all(dir).unwrap();
}

/// A FIFO that is there from the start is refused from the path's metadata, before an open that
/// would wait for a writer.
#[cfg(unix)]
#[test]
fn a_fifo_is_refused_without_waiting_for_a_writer() {
    let dir = scratch();
    let path = dir.join("relay.toml");
    let made = std::process::Command::new("mkfifo").arg(&path).status();
    if !made.is_ok_and(|status| status.success()) {
        eprintln!("SKIPPED: mkfifo is not available");
        return;
    }
    assert!(refusal(&path).contains("is not a regular file"));
    fs::remove_dir_all(dir).unwrap();
}

/// The path's metadata said regular a moment ago, and the open finds a FIFO, as a swap in between
/// leaves one. The open itself has to refuse it, so the open file's metadata is checked too. A
/// writer is held open here so this test cannot hang if the non-blocking flag were lost: the
/// refusal still comes from the opened file, and not from the path's metadata.
#[cfg(target_os = "linux")]
#[test]
fn the_open_itself_refuses_a_fifo() {
    let dir = scratch();
    let path = dir.join("relay.toml");
    fs::write(&path, "enable_metrics = false\n").unwrap();
    assert!(open_regular(&path).is_ok());
    fs::remove_file(&path).unwrap();
    let made = std::process::Command::new("mkfifo").arg(&path).status();
    if !made.is_ok_and(|status| status.success()) {
        eprintln!("SKIPPED: mkfifo is not available");
        return;
    }
    // O_RDWR on a FIFO does not wait on Linux, so there is a writer for the whole test.
    let _writer = rustix::fs::open(&path, rustix::fs::OFlags::RDWR, rustix::fs::Mode::empty())
        .expect("a FIFO with a writer");
    let message = open_regular(&path).unwrap_err().to_string();
    assert!(message.contains("is not a regular file"), "{message}");
    fs::remove_dir_all(dir).unwrap();
}
