use crate::{Mode, relay_args};

#[test]
fn iroh_relay_is_started_on_its_descriptor_zero_and_nothing_else() {
    assert_eq!(relay_args(Mode::Closed), ["--config-path=/proc/self/fd/0"]);
    assert_eq!(relay_args(Mode::Open), ["--config-path=/proc/self/fd/0"]);
    // --dev is how iroh-relay 1.4.1 hands the QUIC server its certificate with the relay half off.
    assert_eq!(
        relay_args(Mode::Rendezvous),
        ["--dev", "--config-path=/proc/self/fd/0"]
    );
}

#[cfg(target_os = "linux")]
mod on_linux {
    use std::fs::{self, File};
    use std::io::{ErrorKind, Read, Write};
    use std::os::fd::AsRawFd;
    use std::path::PathBuf;

    use rustix::fs::{SealFlags, fcntl_get_seals};

    use crate::hand_over;
    use crate::handover::{check_copy_at, sealed_copy};

    const TEXT: &str = "enable_metrics = false\n[access]\nallowlist = []\n";

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "sluice-relay-guard-handover-{}-{name}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The seals ADR 0021 names: no write, no growth, no shrink, and no more seals.
    #[test]
    fn the_copy_carries_the_four_seals() {
        let copy = sealed_copy(TEXT).unwrap();
        let seals = fcntl_get_seals(&copy).unwrap();
        for seal in [
            SealFlags::WRITE,
            SealFlags::GROW,
            SealFlags::SHRINK,
            SealFlags::SEAL,
        ] {
            assert!(seals.contains(seal), "{seals:?}");
        }
    }

    /// The copy is a regular file that reads back the checked bytes through `/proc`, and a write or
    /// a truncate is refused. The write and truncate are what an editor, `cat >` or a generator
    /// script do to the file a config was mounted from.
    #[test]
    fn the_sealed_copy_refuses_a_write_and_a_truncate_and_reads_back_through_proc() {
        let copy = sealed_copy(TEXT).unwrap();
        let path = format!("/proc/self/fd/{}", copy.as_raw_fd());
        assert!(fs::metadata(&path).unwrap().is_file());
        assert_eq!(fs::read_to_string(&path).unwrap(), TEXT);

        let mut handle = &copy;
        let write = handle.write(b"x");
        assert_eq!(write.unwrap_err().kind(), ErrorKind::PermissionDenied);
        let shrink = copy.set_len(0);
        assert_eq!(shrink.unwrap_err().kind(), ErrorKind::PermissionDenied);
        let grow = copy.set_len(TEXT.len() as u64 + 1);
        assert_eq!(grow.unwrap_err().kind(), ErrorKind::PermissionDenied);
        // Nothing of the above changed the bytes.
        assert_eq!(fs::read_to_string(&path).unwrap(), TEXT);
    }

    /// The check of descriptor 0: a path that names another file is refused, and so is a path that
    /// names the copy but holds other bytes.
    #[test]
    fn the_check_refuses_another_file_and_other_bytes() {
        let dir = scratch("other");
        let other = dir.join("other.toml");
        fs::write(&other, "enable_metrics = true\n").unwrap();
        let copy = sealed_copy(TEXT).unwrap();
        let message = check_copy_at(&other, &copy, TEXT).unwrap_err().to_string();
        assert!(message.contains("is not the sealed copy"), "{message}");
        assert!(message.contains("Nothing was started"), "{message}");

        let path = format!("/proc/self/fd/{}", copy.as_raw_fd());
        let message = check_copy_at(std::path::Path::new(&path), &copy, "other")
            .unwrap_err()
            .to_string();
        assert!(message.contains("does not hold the bytes"), "{message}");
        fs::remove_dir_all(dir).unwrap();
    }

    /// A missing `/proc`, as in a runtime that mounts none, is refused with what iroh-relay would
    /// have done instead.
    #[test]
    fn a_path_that_does_not_exist_is_refused_with_what_iroh_relay_would_do() {
        let dir = scratch("missing");
        let copy = sealed_copy(TEXT).unwrap();
        let message = check_copy_at(&dir.join("fd/0"), &copy, TEXT)
            .unwrap_err()
            .to_string();
        assert!(message.contains("cannot read"), "{message}");
        assert!(
            message.contains("would run open with no limit"),
            "{message}"
        );
        assert!(!message.contains('\n'), "one line: {message}");
        fs::remove_dir_all(dir).unwrap();
    }

    /// The whole handover, on this process: descriptor 0 becomes the sealed copy, its link names
    /// the memfd, and it reads back the checked bytes. This is what the container's PID 1 gets.
    #[test]
    fn the_handover_puts_the_sealed_copy_on_descriptor_zero() {
        let dir = scratch("handover");
        let path = dir.join("relay.toml");
        fs::write(&path, TEXT).unwrap();
        let file = File::open(&path).unwrap();
        hand_over(file, TEXT).unwrap();
        let link = fs::read_link("/proc/self/fd/0").unwrap();
        assert!(
            link.to_string_lossy().contains("memfd:sluice-relay-config"),
            "{}",
            link.display()
        );
        let mut read = String::new();
        File::open("/proc/self/fd/0")
            .unwrap()
            .read_to_string(&mut read)
            .unwrap();
        assert_eq!(read, TEXT);
        fs::remove_dir_all(dir).unwrap();
    }
}

#[cfg(not(target_os = "linux"))]
#[test]
fn the_handover_refuses_where_memfd_and_proc_are_missing() {
    let file = std::fs::File::open(std::env::current_exe().unwrap()).unwrap();
    let message = crate::hand_over(file, "enable_metrics = false\n")
        .unwrap_err()
        .to_string();
    assert!(message.contains("Linux only"), "{message}");
    assert!(message.contains("Nothing was started"), "{message}");
}
