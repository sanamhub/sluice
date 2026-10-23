use crate::{Mode, relay_args};

#[test]
fn iroh_relay_is_started_on_its_descriptor_zero_and_nothing_else() {
    assert_eq!(relay_args(Mode::Closed), ["--config-path=/proc/self/fd/0"]);
    assert_eq!(relay_args(Mode::Open), ["--config-path=/proc/self/fd/0"]);
    // --dev is how iroh-relay 1.4.0 hands the QUIC server its certificate with the relay half off.
    assert_eq!(
        relay_args(Mode::Rendezvous),
        ["--dev", "--config-path=/proc/self/fd/0"]
    );
}

#[cfg(unix)]
mod on_unix {
    use std::fs::{self, File};
    use std::path::{Path, PathBuf};

    use crate::{check_handover, names_the_same_file};

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "sluice-relay-guard-handover-{}-{name}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A path that names the open file, which is what `/proc/self/fd/<n>` does where it exists.
    #[test]
    fn a_path_that_names_the_open_file_passes() {
        let dir = scratch("same");
        let path = dir.join("relay.toml");
        fs::write(&path, "enable_metrics = false\n").unwrap();
        let file = File::open(&path).unwrap();
        assert!(names_the_same_file(&path, &file).is_ok());
        fs::remove_dir_all(dir).unwrap();
    }

    /// The runtime without a `/proc`, or a volume over it: the path iroh-relay would read is not
    /// there, so it would run open.
    #[test]
    fn a_path_that_does_not_exist_is_refused_with_what_iroh_relay_would_do() {
        let dir = scratch("missing");
        let file = File::open(std::env::current_exe().unwrap()).unwrap();
        let message = names_the_same_file(&dir.join("fd/0"), &file)
            .unwrap_err()
            .to_string();
        assert!(message.contains("cannot be read"), "{message}");
        assert!(
            message.contains("would run open with no limit"),
            "{message}"
        );
        assert!(!message.contains('\n'), "one line: {message}");
        fs::remove_dir_all(dir).unwrap();
    }

    /// A path that resolves, but to another file: a `/proc` of another process, or a bind mount
    /// over `/proc/self/fd`.
    #[test]
    fn a_path_that_names_another_file_is_refused() {
        let dir = scratch("other");
        let checked = dir.join("checked.toml");
        let other = dir.join("other.toml");
        fs::write(&checked, "a\n").unwrap();
        fs::write(&other, "a\n").unwrap();
        let file = File::open(&checked).unwrap();
        let message = names_the_same_file(&other, &file).unwrap_err().to_string();
        assert!(
            message.contains("is not the config the guard checked"),
            "{message}"
        );
        fs::remove_dir_all(dir).unwrap();
    }

    /// The real check, on the descriptor of a file this process holds. Where `/proc` is not
    /// mounted the same call must refuse, so both outcomes are asserted against the mount.
    #[test]
    fn the_check_follows_whether_proc_is_there() {
        let dir = scratch("proc");
        let path = dir.join("relay.toml");
        fs::write(&path, "x\n").unwrap();
        let file = File::open(&path).unwrap();
        let proc_is_there = Path::new("/proc/self/fd").is_dir();
        assert_eq!(check_handover(&file).is_ok(), proc_is_there);
        fs::remove_dir_all(dir).unwrap();
    }
}
