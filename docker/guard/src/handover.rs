//! How the bytes the guard checked reach iroh-relay.
//!
//! iroh-relay opens its config path itself and reads it from the start (main.rs:542-567). The path
//! cannot be trusted with the bytes: a rename between the guard's read and the relay's starts a
//! file nobody checked, and a write that truncates the file in place leaves an empty one, which
//! iroh-relay reads as its open default. So the guard copies the checked bytes into a `memfd`,
//! seals it against writes and size changes, puts that copy on descriptor 0 itself, and starts
//! iroh-relay with `--config-path=/proc/self/fd/0` (ADR 0021).
//!
//! iroh-relay tests its config path with `exists()`, which is false on any error, and then runs
//! its open default. So before the guard replaces itself it proves that `/proc/self/fd/0` names
//! the copy by device and inode, and reads the checked bytes back through it. A runtime without a
//! usable `/proc` stops here with exit 69, and starts nothing.

use std::ffi::OsString;

/// The path iroh-relay is told to read: its own descriptor 0, which holds the sealed copy of the
/// bytes the guard checked. A `/proc/self/fd` link opens what the descriptor holds, from its
/// start. It is a path of the kernel's and not `/dev/stdin`, which is a link the container runtime
/// makes and some runtimes do not.
///
/// ```
/// assert_eq!(sluice_relay_guard::CONFIG_FD_PATH, "/proc/self/fd/0");
/// ```
pub const CONFIG_FD_PATH: &str = "/proc/self/fd/0";

/// The name the kernel shows in `/proc/<pid>/fd/0` for the sealed copy of the config.
///
/// ```
/// assert_eq!(sluice_relay_guard::MEMFD_NAME, "sluice-relay-config");
/// ```
pub const MEMFD_NAME: &str = "sluice-relay-config";

/// The arguments iroh-relay is started with after the sealed copy is on descriptor 0:
/// `--config-path=/proc/self/fd/0`, and nothing from the command line the guard was given.
///
/// ```
/// assert_eq!(
///     sluice_relay_guard::relay_args(),
///     ["--config-path=/proc/self/fd/0"]
/// );
/// ```
pub fn relay_args() -> Vec<OsString> {
    vec![OsString::from(format!("--config-path={CONFIG_FD_PATH}"))]
}

#[cfg(target_os = "linux")]
pub use linux::hand_over;
#[cfg(all(test, target_os = "linux"))]
pub(crate) use linux::{check_copy_at, sealed_copy};

#[cfg(target_os = "linux")]
mod linux {
    use std::fs::{self, File};
    use std::io::{Seek, SeekFrom, Write};
    use std::os::unix::fs::MetadataExt;
    use std::path::Path;

    use rustix::fs::{MemfdFlags, SealFlags, fcntl_add_seals, memfd_create};
    use rustix::stdio::dup2_stdin;

    use super::{CONFIG_FD_PATH, MEMFD_NAME};
    use crate::Refusal;

    /// The seals the copy carries: no write, no growth, no shrink, and no more seals.
    pub(crate) const SEALS: SealFlags = SealFlags::WRITE
        .union(SealFlags::GROW)
        .union(SealFlags::SHRINK)
        .union(SealFlags::SEAL);

    /// ADR 0021's handover, in its order: copy the checked bytes into a sealed `memfd`, put that
    /// copy on descriptor 0, close the file the guard opened, and prove that `/proc/self/fd/0`
    /// names the copy and reads back the checked bytes. Any failure is a refusal that says which
    /// step failed, and nothing is started.
    ///
    /// ```
    /// let path = std::env::temp_dir().join("guard-handover-doc.toml");
    /// std::fs::write(&path, "enable_metrics = false\n").unwrap();
    /// let file = std::fs::File::open(&path).unwrap();
    /// let handed = sluice_relay_guard::hand_over(file, "enable_metrics = false\n");
    /// // Linux puts the sealed copy on descriptor 0; another system refuses and starts nothing.
    /// assert_eq!(handed.is_ok(), cfg!(target_os = "linux"));
    /// std::fs::remove_file(path).unwrap();
    /// ```
    pub fn hand_over(config: File, text: &str) -> Result<(), Refusal> {
        let copy = sealed_copy(text)?;
        dup2_stdin(&copy).map_err(|err| failed("put the sealed copy on descriptor 0", &err))?;
        // The file the operator gave is closed before the check and the exec: nothing after this
        // reads it, and a descriptor left open could be reused by the relay.
        drop(config);
        check_copy_at(Path::new(CONFIG_FD_PATH), &copy, text)
    }

    /// The checked bytes in a `memfd` that is sealed against a write, a growth, a shrink and a
    /// further seal, and is back at its start. The copy is made before any seal, because
    /// `F_SEAL_WRITE` refuses the write that fills it.
    pub(crate) fn sealed_copy(text: &str) -> Result<File, Refusal> {
        let fd = memfd_create(MEMFD_NAME, MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING)
            .map_err(|err| failed("create the sealed copy of the config", &err))?;
        let mut copy = File::from(fd);
        copy.write_all(text.as_bytes())
            .map_err(|err| failed("write the checked config into the sealed copy", &err))?;
        fcntl_add_seals(&copy, SEALS).map_err(|err| failed("seal the copy of the config", &err))?;
        copy.seek(SeekFrom::Start(0))
            .map_err(|err| failed("rewind the sealed copy", &err))?;
        Ok(copy)
    }

    /// That `path` names the file `copy` is open on, by device and inode, and that reading through
    /// it gives exactly `text`. This is the check of descriptor 0 before the exec.
    pub(crate) fn check_copy_at(path: &Path, copy: &File, text: &str) -> Result<(), Refusal> {
        let shown = path.display();
        let open = copy
            .metadata()
            .map_err(|err| failed("read the sealed copy's metadata", &err))?;
        let named = fs::metadata(path).map_err(|err| {
            Refusal::new(format!(
                "cannot read {shown} ({err}), so iroh-relay would not find its config there and \
                 would run open with no limit: mount /proc, or run the image with one. Nothing \
                 was started"
            ))
        })?;
        if (named.dev(), named.ino()) != (open.dev(), open.ino()) {
            return Err(Refusal::new(format!(
                "{shown} is not the sealed copy of the config, so iroh-relay would read another \
                 file or none and run open with no limit. Nothing was started"
            )));
        }
        let read = fs::read(path).map_err(|err| failed("read the sealed copy back", &err))?;
        if read != text.as_bytes() {
            return Err(Refusal::new(format!(
                "{shown} does not hold the bytes the guard checked, so iroh-relay would start a \
                 config nobody checked. Nothing was started"
            )));
        }
        Ok(())
    }

    /// A refusal that names the step that failed. Every message ends the same way, so an operator
    /// reading the container log knows the guard did not start the relay.
    fn failed(what: &str, err: &dyn std::fmt::Display) -> Refusal {
        Refusal::new(format!("cannot {what}: {err}. Nothing was started"))
    }
}

#[cfg(not(target_os = "linux"))]
pub use elsewhere::hand_over;

#[cfg(not(target_os = "linux"))]
mod elsewhere {
    use std::fs::File;

    use crate::Refusal;

    /// The images are Linux only: the sealed copy needs `memfd_create` and `/proc/self/fd`. On any
    /// other system the config has been checked, and the guard stops rather than hand a config
    /// path iroh-relay could find empty, which it runs as its open default.
    pub fn hand_over(_config: File, _text: &str) -> Result<(), Refusal> {
        Err(Refusal::new(
            "cannot hand iroh-relay a sealed copy of the config: memfd_create and /proc/self/fd \
             are Linux only. Nothing was started",
        ))
    }
}
