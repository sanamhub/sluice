use std::fs;
use std::io::Read;

use crate::{Mode, open_config, relay_args};

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

#[test]
fn iroh_relay_is_started_on_its_standard_input_and_nothing_else() {
    assert_eq!(relay_args(Mode::Closed), ["--config-path=/dev/stdin"]);
    assert_eq!(relay_args(Mode::Open), ["--config-path=/dev/stdin"]);
    // --dev is how iroh-relay 1.4.0 hands the QUIC server its certificate with the relay half off.
    assert_eq!(
        relay_args(Mode::Rendezvous),
        ["--dev", "--config-path=/dev/stdin"]
    );
}

/// A rename over the path after the check, as a ConfigMap update does, changes nothing the relay
/// reads. On Linux `/dev/stdin` is `/proc/self/fd/0`, and opening a `/proc/self/fd` link opens the
/// file the descriptor holds, so the descriptor stands in for the relay's standard input here.
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

/// A FIFO is refused from the path's metadata, before an open that would wait for a writer.
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
