use std::fs;

use crate::read_config;

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

#[test]
fn a_path_that_does_not_exist_is_refused_because_iroh_would_run_open() {
    let dir = scratch();
    let message = read_config(&dir.join("relay.toml"))
        .unwrap_err()
        .to_string();
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
    let message = read_config(&dir).unwrap_err().to_string();
    assert!(message.contains("is not a regular file"), "{message}");
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_regular_file_is_read_as_it_is() {
    let dir = scratch();
    let path = dir.join("relay.toml");
    fs::write(&path, "[access]\nallowlist = []\n").unwrap();
    assert_eq!(read_config(&path).unwrap(), "[access]\nallowlist = []\n");
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_file_that_is_not_utf8_is_refused() {
    let dir = scratch();
    let path = dir.join("relay.toml");
    fs::write(&path, b"[access]\nallowlist = [\"\xff\"]\n").unwrap();
    let message = read_config(&path).unwrap_err().to_string();
    assert!(message.contains("is not UTF-8 text"), "{message}");
    fs::remove_dir_all(dir).unwrap();
}
