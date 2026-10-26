//! The store's home on disk: the folder, its layout version, the one-writer lock and the identity
//! key. Nothing here reads a config or touches the network, so opening a home is cheap.

mod atomic;
pub mod doctor;
mod folder;
mod identity;
mod layout;
mod lock;
// The only unsafe code in the crate. Everything else stays under the crate's `deny`.
#[cfg(windows)]
#[allow(unsafe_code)]
pub(crate) mod private_windows;
mod setup;

use std::env;
use std::ffi::OsString;
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};

pub(crate) use atomic::write_atomic_secret;
pub use identity::Identity;

use crate::Error;

/// The layout of a home:
///
/// ```text
/// identity.key   Ed25519 secret, mode 0600 on Unix
/// VERSION        layout version, as decimal text and a newline
/// lock           locked while a process uses the home
/// lock.pid       the pid of that process, for the message of a refused open. Removed when the
///                process lets go cleanly, so one left behind marks a process that did not
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
    /// The path the caller gave, which [`Home::path`] returns and messages name.
    path: PathBuf,
    /// `path` with every link followed: where every file of the home is read and written.
    root: PathBuf,
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
    /// missing, with mode 0700 on Unix, and takes the exclusive lock on it. A home folder that is
    /// already there and holds nothing at all is made private first, as a new one is: mode 0700 on
    /// Unix, and the private access list on Windows. One that holds anything keeps its access.
    ///
    /// `path` may be a link, a Windows junction or a Unix symlink, to the folder to use. It is
    /// followed once, here, and the folder it leads to is the one made private and the one every
    /// file goes in. Messages name `path` as given.
    ///
    /// - Something that appears in an empty home while it is made private fails with
    ///   [`Error::Policy`] naming the home unless sluice makes it and this user made it. A key or
    ///   its temporary file is never accepted, even this user's, and neither is a `blobs`, `db` or
    ///   `logs` that is a link.
    /// - A home, or a `blobs`, `db` or `logs` folder, that another user owns fails with
    ///   [`Error::Policy`] naming it, on every open: its owner could change whatever sluice sets.
    ///   Root on Unix, and `Administrators` and `SYSTEM` on Windows, count as this user, since they
    ///   can already do anything to the files, and a home a backup tool restored still opens.
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
        let root = setup::create_folders(path)?;
        let given = |e| as_given(e, &root, path);
        let lock = lock::take(&root).map_err(given)?;
        // Again under the lock, which is the look that counts: the first one was only to refuse
        // early, and another process can have migrated the home in between.
        layout::settle(&root).map_err(given)?;
        Ok(Self {
            path: path.to_path_buf(),
            root,
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

    /// The UDP port the last send bound, if one was saved. A restarted sender tries it first so an
    /// old ticket's direct addresses still work; a port that is taken falls back to any port.
    pub(crate) fn saved_port(&self) -> Option<u16> {
        let text = fs::read_to_string(self.path.join("last-port")).ok()?;
        text.trim().parse().ok()
    }

    /// Remembers the port of this send. Best effort: the ticket is already usable, and a home
    /// that cannot take the file is not a reason to fail the transfer.
    pub(crate) fn save_port(&self, port: u16) {
        let _ = atomic::write_atomic(&self.path, "last-port", format!("{port}\n").as_bytes());
    }

    /// The identity of this home. The key is made the first time this is called, not when the home
    /// is opened, and never replaced after that except by [`Home::rotate_identity`]. A key file
    /// of the wrong length is an [`Error::Storage`] and is left alone.
    ///
    /// An existing key is opened without following a link and without waiting for a writer, and
    /// refused with [`Error::Policy`] if it is a link, is not a regular file, has more than one
    /// name, or belongs to another user. A FIFO at the name is refused as not a regular file rather
    /// than waited on. The message names the file and, for another user's key, the command that
    /// takes it back. A key this user cannot open at all is an [`Error::Storage`] that names what
    /// could not be opened.
    ///
    /// On Unix, an existing key that group or other has any access to is refused with
    /// [`Error::Policy`] that names the path and its mode and says to run `chmod 600`. The file is
    /// not changed. [`Home::rotate_identity`] does not read the old key, so it works on one like
    /// that and leaves a key with mode 0600.
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
        identity::load_or_create(&self.root).map_err(|e| as_given(e, &self.root, &self.path))
    }

    /// Replaces the identity with a new one and returns it. Every pairing and every unexpired
    /// ticket that names the old endpoint id stops working, so a caller asks first.
    ///
    /// The key that is there is checked the way [`Home::identity`] checks it before anything is
    /// written: a link, a file that is not regular, one with more than one name, or one another
    /// user owns is [`Error::Policy`] and is left as it is.
    ///
    /// A key this user cannot open, as mode 0000 on Unix makes it, is replaced rather than refused:
    /// the new key is written to a temporary file and renamed over the name, so nothing is written
    /// through the old file. The entry is checked by name for the same four refusals, and a key
    /// that fails one is still left as it is.
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
        identity::rotate(&self.root).map_err(|e| as_given(e, &self.root, &self.path))
    }
}

/// Removes `lock.pid` while the lock is still held, before the file that holds it closes. A
/// `lock.pid` that is left with nobody holding the lock is then the mark of a process that ended
/// without letting go, which `sluice doctor` reports as stale. A process that dies leaves it, and
/// the next open writes its own pid over it.
impl Drop for Home {
    fn drop(&mut self) {
        // Best effort: a pid file that cannot be removed only makes the next doctor warn.
        let _ = std::fs::remove_file(self.root.join("lock.pid"));
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

/// An error about the home or a file in it, told with the path the caller gave rather than the
/// one its links lead to. Every message here names a path with `display`, so the resolved path is
/// found as text and put back the way the caller wrote it.
fn as_given(err: Error, root: &Path, given: &Path) -> Error {
    match err {
        Error::Storage(message) => Error::Storage(show(&message, root, given)),
        Error::Policy(message) => Error::Policy(show(&message, root, given)),
        other => other,
    }
}

/// `text` with the resolved `root` written as the caller gave it, `given`. On Windows `root` is
/// the verbatim form, `\\?\C:\...`, which the user never has to read.
///
/// - A separator at the end of `given` is dropped, or `blobs` would be joined to it twice.
/// - Only a whole path is replaced: the one that ends there, or goes on with a separator. `/h`
///   inside `/hx/lock` is another folder's name.
/// - It runs once over `text`, so a `given` that holds `root`, as `/h/../h` holds `/h`, is not
///   replaced again inside what was just put there.
pub(crate) fn show(text: &str, root: &Path, given: &Path) -> String {
    let root = root.display().to_string();
    let shown = given.display().to_string();
    let given = match shown.trim_end_matches(['/', '\\']) {
        "" => shown.as_str(),
        trimmed => trimmed,
    };
    if root == given || root.is_empty() {
        return text.to_string();
    }
    // A path is replaced only as a whole. A character that can go on with a path name, a dot
    // included, means the text holds another path: `/srv/real.old` is not `/srv/real`. A
    // separator, punctuation, a space or the end of the text ends it.
    let path_char = |c: Option<char>| {
        c.is_some_and(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | '~'))
    };
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    let mut before = None;
    while let Some(at) = rest.find(&root) {
        let (head, tail) = rest.split_at(at);
        let after = tail[root.len()..].chars().next();
        out.push_str(head);
        let whole = !path_char(head.chars().next_back().or(before)) && !path_char(after);
        out.push_str(if whole { given } else { &root });
        rest = &tail[root.len()..];
        before = root.chars().next_back();
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests;
