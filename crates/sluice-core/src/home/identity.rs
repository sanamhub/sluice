//! The user's identity key: one Ed25519 secret, kept in `identity.key`.

use std::fmt;
use std::fs::File;
use std::io::{ErrorKind, Read};
use std::path::Path;

use ed25519_dalek::SigningKey;
use zeroize::Zeroizing;

use super::atomic::write_atomic_secret;
use super::storage;
use crate::Error;

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
        Some(file) => {
            check_key(&file, &path, owner)?;
            refuse_readable_by_others(&file, &path)?;
            read_seed(file, &path).map(|seed| Identity::from_seed(&seed))
        }
        None => create(dir),
    }
}

/// Writes a new key over the old one and returns it. An existing key is checked as
/// [`load_or_create`] checks it first, so rotating never takes over a key another user owns.
pub(super) fn rotate(dir: &Path) -> Result<Identity, Error> {
    rotate_with(dir, owner_is_accepted)
}

/// [`rotate`] with the owner check as a parameter, for the same reason as [`load_or_create_with`].
pub(super) fn rotate_with(
    dir: &Path,
    owner: fn(&File) -> std::io::Result<bool>,
) -> Result<Identity, Error> {
    let path = dir.join(KEY_FILE);
    if let Some(file) = open_key(&path)? {
        check_key(&file, &path, owner)?;
    }
    create(dir)
}

/// The key's owner, its kind and its link count, read from the handle it is read through. A name
/// is not proof of what it holds: the key is opened once, without following a link, and every
/// check and the read after them are of that one object.
fn check_key(
    file: &File,
    path: &Path,
    owner: fn(&File) -> std::io::Result<bool>,
) -> Result<(), Error> {
    let meta = file.metadata().map_err(|e| storage("inspect", path, e))?;
    if meta.file_type().is_symlink() {
        return Err(Error::Policy(is_a_link(path)));
    }
    if !meta.is_file() {
        return Err(Error::Policy(not_a_regular_file(path)));
    }
    let links = links(file, &meta).map_err(|e| storage("inspect", path, e))?;
    if links != 1 {
        return Err(Error::Policy(too_many_links(path, links)));
    }
    match owner(file) {
        Ok(true) => Ok(()),
        Ok(false) => Err(Error::Policy(owned_by_other(path))),
        Err(e) => Err(storage("read the owner of", path, e)),
    }
}

/// The message for a key another user owns, which says how to take it back. Doctor says the same
/// thing about the same key.
fn owned_by_other(path: &Path) -> String {
    format!(
        "{} belongs to another user, who could change it whatever sluice sets. {}, or move it \
         elsewhere and run `sluice id` for a new identity",
        path.display(),
        take_back(path)
    )
}

/// The platform's own commands for taking a file back from another user.
#[cfg(unix)]
fn take_back(path: &Path) -> String {
    format!(
        "Run `sudo chown $(id -un) {}` to take it back",
        path.display()
    )
}

/// The platform's own commands for taking a file back from another user.
#[cfg(windows)]
fn take_back(path: &Path) -> String {
    let shown = path.display();
    format!(
        "Run `takeown /f {shown}` and `icacls {shown} /inheritance:r /grant:r %USERNAME%:F` to \
         take it back"
    )
}

#[cfg(not(any(unix, windows)))]
fn take_back(_path: &Path) -> String {
    "Take ownership of it back".to_string()
}

fn is_a_link(path: &Path) -> String {
    format!(
        "{} is a link and not the identity key: remove it, and run `sluice id` for a new identity",
        path.display()
    )
}

fn not_a_regular_file(path: &Path) -> String {
    format!(
        "{} is not a regular file, and the identity key must be one: move it elsewhere, and run \
         `sluice id` for a new identity",
        path.display()
    )
}

fn too_many_links(path: &Path, links: u64) -> String {
    format!(
        "{} has {links} links, and the identity key must have one: remove the other names, or \
         move it elsewhere and run `sluice id` for a new identity",
        path.display()
    )
}

/// The number of names the key has, from the handle it was opened through.
#[cfg(unix)]
fn links(_file: &File, meta: &std::fs::Metadata) -> std::io::Result<u64> {
    use std::os::unix::fs::MetadataExt;
    Ok(meta.nlink())
}

#[cfg(windows)]
fn links(file: &File, _meta: &std::fs::Metadata) -> std::io::Result<u64> {
    super::private_windows::links_of(file)
}

#[cfg(not(any(unix, windows)))]
fn links(_file: &File, _meta: &std::fs::Metadata) -> std::io::Result<u64> {
    Ok(1)
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

/// The key, open for reading, or `None` when there is none. On Unix the open does not follow a
/// link, and a link is refused by name: a link at the key's name would put the store's identity
/// wherever it points. Windows opens the reparse point itself, and [`check_key`] refuses it.
#[cfg(unix)]
fn open_key(path: &Path) -> Result<Option<File>, Error> {
    use rustix::fs::{Mode, OFlags};
    match rustix::fs::open(path, OFlags::NOFOLLOW | OFlags::CLOEXEC, Mode::empty()) {
        Ok(fd) => Ok(Some(File::from(fd))),
        Err(e) if e == rustix::io::Errno::LOOP => Err(Error::Policy(is_a_link(path))),
        Err(e) if e == rustix::io::Errno::NOENT => Ok(None),
        Err(e) => Err(storage("open", path, e.into())),
    }
}

#[cfg(windows)]
fn open_key(path: &Path) -> Result<Option<File>, Error> {
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
        Ok(file) => Ok(Some(file)),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
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

/// Refuses a key that group or other can read, write or run. The mode comes from the handle that
/// the key will be read through, so the file checked is the file read, even if the path is
/// swapped in between. It only reads: loosening it was somebody's choice, and a file that sluice
/// changed would hide that the key may have been exposed. [`rotate`] never reads the old key, so it
/// is the way out that does not need the file fixed first.
#[cfg(unix)]
fn refuse_readable_by_others(file: &File, path: &Path) -> Result<(), Error> {
    use std::os::unix::fs::PermissionsExt;
    let mode = file
        .metadata()
        .map_err(|e| storage("inspect", path, e))?
        .permissions()
        .mode()
        & 0o7777;
    if mode & 0o077 == 0 {
        return Ok(());
    }
    Err(Error::Policy(format!(
        "{} has mode {mode:04o}, so other users may be able to read the identity key. \
         Run `chmod 600 {}` and try again",
        path.display(),
        path.display()
    )))
}

/// Windows has no mode bits. A new key gets its own access list when it is made, and one that
/// was loosened afterwards is not looked for, because that would need a security call on every
/// load.
#[cfg(not(unix))]
fn refuse_readable_by_others(_file: &File, _path: &Path) -> Result<(), Error> {
    Ok(())
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
