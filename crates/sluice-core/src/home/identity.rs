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
pub(super) fn load_or_create(dir: &Path) -> Result<Identity, Error> {
    let path = dir.join(KEY_FILE);
    match File::open(&path) {
        Ok(file) => {
            refuse_readable_by_others(&file, &path)?;
            read_seed(file, &path).map(|seed| Identity::from_seed(&seed))
        }
        Err(e) if e.kind() == ErrorKind::NotFound => create(dir),
        Err(e) => Err(storage("open", &path, e)),
    }
}

/// Writes a new key over the old one and returns it.
pub(super) fn rotate(dir: &Path) -> Result<Identity, Error> {
    create(dir)
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
