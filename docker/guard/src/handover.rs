//! How the file the guard checked reaches iroh-relay.
//!
//! iroh-relay opens its config path itself (main.rs:542-567). If the guard passed it the path,
//! a rename between the guard's read and the relay's, such as a Kubernetes ConfigMap update, would
//! start a file nobody checked. So the guard keeps the file it read open, and starts iroh-relay
//! with that file as its standard input, told to read `--config-path=/proc/self/fd/0` (ADR 0020).
//!
//! iroh-relay tests its config path with `exists()`, which is false on any error, and then runs
//! its open default. So a path that does not resolve in some runtime is not a startup error but an
//! open relay. [`check_handover`] proves, before the guard replaces itself, that `/proc` is there
//! and names this process's descriptors.

use std::ffi::OsString;

use crate::Mode;

/// The path iroh-relay is told to read: its own descriptor 0, which is the file the guard
/// checked. A `/proc/self/fd` link opens the file the descriptor holds, from its start, so a
/// rename of the path after the check cannot change what the relay reads. It is a path of the
/// kernel's and not `/dev/stdin`, which is a link the container runtime makes and some runtimes do
/// not.
///
/// ```
/// assert_eq!(sluice_relay_guard::CONFIG_FD_PATH, "/proc/self/fd/0");
/// ```
pub const CONFIG_FD_PATH: &str = "/proc/self/fd/0";

/// The arguments iroh-relay is started with, after the guard has checked the file it hands over
/// as standard input: `--config-path=/proc/self/fd/0`, and nothing from the command line the guard
/// was given.
///
/// In rendezvous mode `--dev` comes first. iroh-relay 1.4.0 gives the QUIC server of address
/// discovery a certificate only on the path `--dev` takes when the relay half is off
/// (main.rs:593-600 and 703-721), and without it refuses the config with `TLS not configured`.
/// Its other effects, plain HTTP for the relay and a default HTTP port, have nothing to act on
/// with no relay half, which the guard has checked is off. The guard refuses `--dev` from the
/// command line, in every mode.
///
/// ```
/// use sluice_relay_guard::{Mode, relay_args};
///
/// assert_eq!(relay_args(Mode::Closed), ["--config-path=/proc/self/fd/0"]);
/// assert_eq!(relay_args(Mode::Open), ["--config-path=/proc/self/fd/0"]);
/// assert_eq!(relay_args(Mode::Rendezvous), ["--dev", "--config-path=/proc/self/fd/0"]);
/// ```
pub fn relay_args(mode: Mode) -> Vec<OsString> {
    let config = OsString::from(format!("--config-path={CONFIG_FD_PATH}"));
    match mode {
        Mode::Rendezvous => vec![OsString::from("--dev"), config],
        Mode::Closed | Mode::Open => vec![config],
    }
}

#[cfg(unix)]
pub use unix::{check_handover, names_the_same_file};

#[cfg(unix)]
mod unix {
    use std::fs::{self, File};
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::MetadataExt;
    use std::path::Path;

    use crate::Refusal;

    /// Proves that `/proc/self/fd/<n>`, for the descriptor `n` the config is open on, names that
    /// same file, by device and inode. Then `/proc` exists in this runtime and resolves this
    /// process's descriptors, so `--config-path=/proc/self/fd/0` finds the file in iroh-relay,
    /// which keeps it as descriptor 0 across the exec. If not, the refusal says why, and nothing
    /// should be started.
    ///
    /// ```
    /// use sluice_relay_guard::check_handover;
    ///
    /// let file = std::fs::File::open(std::env::current_exe().unwrap()).unwrap();
    /// // Where /proc is mounted, as on Linux, the open file is found by its descriptor.
    /// assert_eq!(check_handover(&file).is_ok(), std::path::Path::new("/proc/self/fd").is_dir());
    /// ```
    pub fn check_handover(config: &File) -> Result<(), Refusal> {
        let path = format!("/proc/self/fd/{}", config.as_raw_fd());
        names_the_same_file(Path::new(&path), config)
    }

    /// Whether `path` resolves to the file `config` is open on: the same device and the same
    /// inode. The reason is in the refusal, and says what `iroh-relay` would have done instead.
    ///
    /// ```
    /// use sluice_relay_guard::names_the_same_file;
    ///
    /// let exe = std::env::current_exe().unwrap();
    /// let file = std::fs::File::open(&exe).unwrap();
    /// assert!(names_the_same_file(&exe, &file).is_ok());
    /// assert!(names_the_same_file(std::path::Path::new("/no/such/path"), &file).is_err());
    /// assert!(names_the_same_file(&std::env::temp_dir(), &file).is_err());
    /// ```
    pub fn names_the_same_file(path: &Path, config: &File) -> Result<(), Refusal> {
        let shown = path.display();
        let open = config
            .metadata()
            .map_err(|err| Refusal::new(format!("cannot read the open config: {err}")))?;
        let named = fs::metadata(path).map_err(|err| {
            Refusal::new(format!(
                "{shown} cannot be read ({err}), so iroh-relay would not find its config there and \
                 would run open with no limit: mount /proc, or run the image with one"
            ))
        })?;
        if (named.dev(), named.ino()) == (open.dev(), open.ino()) {
            return Ok(());
        }
        Err(Refusal::new(format!(
            "{shown} is not the config the guard checked, so iroh-relay would read another file or \
             none, and run open with no limit: this runtime does not give the process its own \
             /proc"
        )))
    }
}
