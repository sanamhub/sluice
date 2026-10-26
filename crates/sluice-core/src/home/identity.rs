//! The user's identity key: one Ed25519 secret, kept in `identity.key`.

use std::fmt;
use std::fs::File;
use std::io::{self, ErrorKind, Read};
use std::path::Path;

use ed25519_dalek::SigningKey;
use zeroize::Zeroizing;

use super::atomic::write_atomic_secret;
use super::storage;
use crate::Error;

mod checks;

const KEY_FILE: &str = "identity.key";
/// The secret seed of an Ed25519 key. It is the same 32 bytes `iroh::SecretKey` holds, so the
/// endpoint id that iroh derives from this file is the one printed here.
const SEED_LEN: usize = 32;

/// The identity of this user: the key that names the endpoint to every peer.
///
/// The secret is private to this crate and is wiped when the value drops. `Debug` prints the
/// endpoint id, which is public, and nothing else.
///
/// # Examples
///
/// ```
/// use sluice_core::Home;
///
/// let dir = tempfile::tempdir()?;
/// let home = Home::open(dir.path())?;
/// let identity = home.identity()?;
/// assert_eq!(identity.endpoint_id_hex().len(), 64);
/// assert!(!format!("{identity:?}").contains("secret"));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub struct Identity {
    key: SigningKey,
}

impl Identity {
    fn from_seed(seed: &[u8; SEED_LEN]) -> Self {
        Self {
            key: SigningKey::from_bytes(seed),
        }
    }

    /// The Ed25519 public key, the 32 bytes that identify the endpoint.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::Home;
    ///
    /// let dir = tempfile::tempdir()?;
    /// let identity = Home::open(dir.path())?.identity()?;
    /// assert_eq!(identity.endpoint_id().len(), 32);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn endpoint_id(&self) -> [u8; 32] {
        self.key.verifying_key().to_bytes()
    }

    /// The endpoint id as 64 lower-case hex characters, which is how iroh prints an `EndpointId`.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::Home;
    ///
    /// let dir = tempfile::tempdir()?;
    /// let identity = Home::open(dir.path())?.identity()?;
    /// let hex = identity.endpoint_id_hex();
    /// assert!(hex.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn endpoint_id_hex(&self) -> String {
        self.endpoint_id()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    /// The secret seed an endpoint needs to prove this identity. Crate-private: a caller outside
    /// the engine gets the public id, never the key.
    pub(crate) fn secret_seed(&self) -> [u8; SEED_LEN] {
        self.key.to_bytes()
    }
}

impl fmt::Debug for Identity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Identity")
            .field("endpoint_id", &self.endpoint_id_hex())
            .finish_non_exhaustive()
    }
}

/// The key in `dir/identity.key`, made first if there is none. An existing file is never replaced
/// here: one of the wrong length is an error, so a damaged key is not quietly swapped for a new
/// identity that every peer would see as a stranger.
///
/// An existing key is opened without following a link, and refused unless it is a regular file
/// with one link, owned by this user or root on Unix, or this user, `Administrators` or `SYSTEM`
/// on Windows. A key another user owns is theirs to change, and a key with two names can be
/// changed through the other one.
pub(super) fn load_or_create(dir: &Path) -> Result<Identity, Error> {
    load_or_create_with(dir, owner_is_accepted)
}

/// [`load_or_create`] with the owner check as a parameter, so a test can be a key another user
/// made.
pub(super) fn load_or_create_with(
    dir: &Path,
    owner: fn(&File) -> std::io::Result<bool>,
) -> Result<Identity, Error> {
    let path = dir.join(KEY_FILE);
    match open_key(&path)? {
        Opened::Key(file) => {
            checks::check_key(&file, &path, owner)?;
            checks::refuse_readable_by_others(&file, &path)?;
            read_seed(file, &path).map(|seed| Identity::from_seed(&seed))
        }
        Opened::Missing => create(dir),
        // The key is there and this user may not read it. Loading refuses it; only rotate writes
        // over a key like that.
        Opened::Denied(e) => Err(storage("open", &path, e)),
    }
}

/// Writes a new key over the old one and returns it. An existing key is checked as
/// [`load_or_create`] checks it first, so rotating never takes over a key another user owns.
///
/// A key this user may not read, as one at mode 0000 is, is rotated rather than refused: the new
/// key goes to a temporary file and is renamed over the name, so nothing is written through the old
/// file. The entry is checked by name before that, and a link, another kind, a second name or
/// another owner is refused in the same words as when the key can be opened.
pub(super) fn rotate(dir: &Path) -> Result<Identity, Error> {
    rotate_with(dir, owner_is_accepted)
}

/// [`rotate`] with the owner check as a parameter, for the same reason as [`load_or_create_with`].
pub(super) fn rotate_with(
    dir: &Path,
    owner: fn(&File) -> std::io::Result<bool>,
) -> Result<Identity, Error> {
    let path = dir.join(KEY_FILE);
    match open_key(&path)? {
        Opened::Key(file) => checks::check_key(&file, &path, owner)?,
        Opened::Missing => {}
        // Checked by name, because the handle that would check it is the one that was refused.
        Opened::Denied(_) => checks::check_unopened(&path)?,
    }
    create(dir)
}

/// What the open of `identity.key` found.
enum Opened {
    /// The key, open for reading.
    Key(File),
    /// There is no file at the name.
    Missing,
    /// The open was refused for access. The key is there, and rotate may write over the name
    /// without reading through it; `Home::identity` refuses it.
    Denied(io::Error),
}

/// Whether the owner of the key `file` is a handle to is this user, or root on Unix, or `SYSTEM`
/// or `Administrators` on Windows. Root and `SYSTEM` can already do anything to the file, so
/// accepting them adds nothing to what they could do.
#[cfg(unix)]
fn owner_is_accepted(file: &File) -> std::io::Result<bool> {
    use std::os::unix::fs::MetadataExt;
    let uid = file.metadata()?.uid();
    Ok(uid == rustix::process::geteuid().as_raw() || uid == 0)
}

#[cfg(windows)]
fn owner_is_accepted(file: &File) -> std::io::Result<bool> {
    super::private_windows::is_owned_by_us_handle(file)
}

#[cfg(not(any(unix, windows)))]
fn owner_is_accepted(_file: &File) -> std::io::Result<bool> {
    Ok(true)
}

/// The key, open for reading, or nothing when there is none. On Unix the open neither follows a
/// link nor waits for a writer, and a link is refused by name: a link at the key's name would put
/// the store's identity wherever it points. A FIFO there would otherwise keep `sluice id` waiting
/// for a writer that never comes, so the open does not block and [`checks::check_key`] refuses it
/// as not a regular file. Windows opens the reparse point itself, and the same check refuses it.
#[cfg(unix)]
fn open_key(path: &Path) -> Result<Opened, Error> {
    use rustix::fs::{Mode, OFlags};
    let flags = OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK;
    match rustix::fs::open(path, flags, Mode::empty()) {
        Ok(fd) => Ok(Opened::Key(File::from(fd))),
        Err(e) if e == rustix::io::Errno::LOOP => Err(Error::Policy(checks::is_a_link(path))),
        Err(e) if e == rustix::io::Errno::NOENT => Ok(Opened::Missing),
        Err(e) if e == rustix::io::Errno::ACCESS || e == rustix::io::Errno::PERM => {
            Ok(Opened::Denied(e.into()))
        }
        Err(e) => Err(storage("open", path, e.into())),
    }
}

#[cfg(windows)]
fn open_key(path: &Path) -> Result<Opened, Error> {
    use std::fs::OpenOptions;
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    };

    match OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
    {
        Ok(file) => Ok(Opened::Key(file)),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(Opened::Missing),
        Err(e) if e.kind() == ErrorKind::PermissionDenied => Ok(Opened::Denied(e)),
        Err(e) => Err(storage("open", path, e)),
    }
}

#[cfg(not(any(unix, windows)))]
fn open_key(path: &Path) -> Result<Option<File>, Error> {
    match File::open(path) {
        Ok(file) => Ok(Some(file)),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        Err(e) => Err(storage("open", path, e)),
    }
}

fn create(dir: &Path) -> Result<Identity, Error> {
    let mut seed = Zeroizing::new([0u8; SEED_LEN]);
    getrandom::fill(&mut seed[..]).map_err(|e| {
        Error::Storage(format!(
            "cannot read random bytes from the operating system: {e}"
        ))
    })?;
    write_atomic_secret(dir, KEY_FILE, &seed[..])?;
    Ok(Identity::from_seed(&seed))
}

/// The 32 bytes of the file. It reads one byte more than a key holds, to tell a file that is too
/// long from one that is exactly right. Nothing of the content goes into an error.
fn read_seed(mut file: File, path: &Path) -> Result<Zeroizing<[u8; SEED_LEN]>, Error> {
    let mut buffer = Zeroizing::new([0u8; SEED_LEN + 1]);
    let mut filled = 0;
    while filled < buffer.len() {
        match file.read(&mut buffer[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(e) => return Err(storage("read", path, e)),
        }
    }
    if filled != SEED_LEN {
        let size = if filled > SEED_LEN {
            "longer than 32 bytes".to_string()
        } else {
            format!("{filled} bytes")
        };
        return Err(Error::Storage(format!(
            "{} is {size} and an identity key is 32. It was left as it is, so remove it by hand \
             if you want a new identity",
            path.display()
        )));
    }
    let mut seed = Zeroizing::new([0u8; SEED_LEN]);
    seed.copy_from_slice(&buffer[..SEED_LEN]);
    Ok(seed)
}

#[cfg(test)]
mod tests;
