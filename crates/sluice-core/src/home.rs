//! The store's home on disk: the folder, its layout version, the one-writer lock and the identity
//! key. Nothing here reads a config or touches the network, so opening a home is cheap.

mod atomic;
mod folder;
mod identity;
mod layout;
mod lock;

use std::env;
use std::ffi::OsString;
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};

pub use identity::Identity;

use crate::Error;

/// The layout of a home:
///
/// ```text
/// identity.key   Ed25519 secret, mode 0600 on Unix
/// VERSION        layout version, as decimal text and a newline
/// lock           locked while a process uses the home
/// lock.pid       the pid of that process, for the message of a refused open
/// blobs/  db/  logs/
/// ```
///
/// A `Home` value holds the lock on its folder, so at most one process has a given home open. The
/// lock is released when the value drops, or when the process ends.
///
/// # Examples
///
/// ```
/// use sluice_core::Home;
///
/// let dir = tempfile::tempdir()?;
/// let home = Home::open(dir.path())?;
/// assert!(dir.path().join("blobs").is_dir());
/// assert_eq!(std::fs::read_to_string(dir.path().join("VERSION"))?, "1\n");
/// // A second open of the same folder is refused while the first is alive.
/// assert!(Home::open(dir.path()).is_err());
/// drop(home);
/// assert!(Home::open(dir.path()).is_ok());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug)]
pub struct Home {
    path: PathBuf,
    // Never read: it is held so the lock stays.
    _lock: File,
}

impl Home {
    /// The folder a home lives in: `$SLUICE_HOME` if it is set and not empty, else `sluice` in the
    /// per-user data folder. That is `$XDG_DATA_HOME` or `~/.local/share` on Linux,
    /// `~/Library/Application Support` on macOS, and the local app data folder on Windows. It is
    /// the local one and not the roaming one, so the identity key does not sync to another
    /// machine. Nothing is created or read.
    ///
    /// Fails with [`Error::Storage`] if there is no `$SLUICE_HOME` and the user has no data folder.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::Home;
    ///
    /// // The answer is `SLUICE_HOME` if that is set, else a folder named `sluice`.
    /// if let Ok(path) = Home::locate() {
    ///     assert!(!path.as_os_str().is_empty());
    /// }
    /// ```
    pub fn locate() -> Result<PathBuf, Error> {
        locate_from(env::var_os("SLUICE_HOME"), folder::data_folder())
    }

    /// Opens the home at `path`, creating it and its `blobs`, `db` and `logs` folders if they are
    /// missing, with mode 0700 on Unix, and takes the exclusive lock on it.
    ///
    /// - A home with a newer layout version than this build reads fails with [`Error::Policy`]
    ///   naming both versions, before anything is created or written.
    /// - A `VERSION` file that does not hold a version fails with [`Error::Storage`].
    /// - If another live process holds the lock, it fails with [`Error::Policy`] naming that
    ///   process where the platform lets the id be read. A lock left by a process that died is not
    ///   held by anyone, so it opens.
    /// - Any other failure of the filesystem is [`Error::Storage`] and names the path.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::{Error, Home};
    ///
    /// let dir = tempfile::tempdir()?;
    /// std::fs::write(dir.path().join("VERSION"), "2\n")?;
    /// let err = Home::open(dir.path()).unwrap_err();
    /// assert!(matches!(err, Error::Policy(_)));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn open(path: &Path) -> Result<Self, Error> {
        // First without the lock and without writing, so a newer home is refused untouched.
        if let Some(found) = layout::read_version(path)? {
            layout::refuse_newer(found)?;
        }
        layout::create_folders(path)?;
        let lock = lock::take(path)?;
        // Again under the lock, which is the look that counts: the first one was only to refuse
        // early, and another process can have migrated the home in between.
        layout::settle(path)?;
        Ok(Self {
            path: path.to_path_buf(),
            _lock: lock,
        })
    }

    /// The folder of this home.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::Home;
    ///
    /// let dir = tempfile::tempdir()?;
    /// let home = Home::open(dir.path())?;
    /// assert_eq!(home.path(), dir.path());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The identity of this home. The key is made the first time this is called, not when the home
    /// is opened, and never replaced after that except by [`Home::rotate_identity`]. A key file
    /// of the wrong length is an [`Error::Storage`] and is left alone.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::Home;
    ///
    /// let dir = tempfile::tempdir()?;
    /// let home = Home::open(dir.path())?;
    /// assert!(!dir.path().join("identity.key").exists());
    /// let first = home.identity()?;
    /// assert!(dir.path().join("identity.key").exists());
    /// assert_eq!(home.identity()?.endpoint_id(), first.endpoint_id());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn identity(&self) -> Result<Identity, Error> {
        identity::load_or_create(&self.path)
    }

    /// Replaces the identity with a new one and returns it. Every pairing and every unexpired
    /// ticket that names the old endpoint id stops working, so a caller asks first.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::Home;
    ///
    /// let dir = tempfile::tempdir()?;
    /// let home = Home::open(dir.path())?;
    /// let before = home.identity()?.endpoint_id();
    /// let after = home.rotate_identity()?.endpoint_id();
    /// assert_ne!(before, after);
    /// assert_eq!(home.identity()?.endpoint_id(), after);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn rotate_identity(&self) -> Result<Identity, Error> {
        identity::rotate(&self.path)
    }
}

/// `$SLUICE_HOME` if it is set and not empty, else `sluice` in the data folder. Split from
/// `locate` so the tests do not have to change the environment of the whole process.
fn locate_from(
    sluice_home: Option<OsString>,
    data_folder: Option<PathBuf>,
) -> Result<PathBuf, Error> {
    match sluice_home {
        Some(path) if !path.is_empty() => Ok(PathBuf::from(path)),
        _ => data_folder.map(|dir| dir.join("sluice")).ok_or_else(|| {
            Error::Storage(
                "cannot find a data folder for this user. Set SLUICE_HOME to the folder to use"
                    .to_string(),
            )
        }),
    }
}

/// `Error::Storage` for a failed filesystem call, with what was being done and where.
fn storage(doing: &str, path: &Path, source: io::Error) -> Error {
    Error::Storage(format!("cannot {doing} {}: {source}", path.display()))
}

#[cfg(test)]
mod tests;
