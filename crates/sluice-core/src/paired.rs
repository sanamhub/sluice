//! The daemons this device has paired with, in `$SLUICE_HOME/paired.json`. A daemon keeps its
//! grants in `devices.json`; this is the other list, the one a device keeps about daemons it may
//! deliver to. A pairing ticket is never stored: only the endpoint id, the addresses and the grant
//! it gave.

use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    str::FromStr,
};

use serde::{Deserialize, Serialize};

use crate::Error;
use crate::device::{Grant, check_label};
use crate::home::atomic::write_atomic;

/// The file inside the home.
const FILE: &str = "paired.json";
/// The version this writer writes and this reader accepts.
const VERSION: u8 = 1;

/// A daemon this device paired with, under a name the person chose.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PairedDaemon {
    /// What this device calls the daemon. The first 8 hex characters of its id until renamed.
    pub name: String,
    /// The daemon's endpoint id, as a pairing ticket named it.
    pub endpoint_id: [u8; 32],
    /// The addresses the ticket carried.
    pub addrs: Vec<SocketAddr>,
    /// The relay the ticket carried, if any.
    pub relay: Option<String>,
    /// What the daemon granted when the ticket was redeemed.
    pub grant: Grant,
    /// When it was redeemed, in UTC milliseconds since the Unix epoch.
    pub paired_ms: u64,
}

/// The file as it is written.
#[derive(Serialize, Deserialize)]
struct Stored {
    version: u8,
    daemons: Vec<StoredDaemon>,
}

#[derive(Serialize, Deserialize)]
struct StoredDaemon {
    name: String,
    endpoint_id: String,
    addrs: Vec<String>,
    relay: Option<String>,
    grant: Grant,
    paired_ms: u64,
}

/// The daemons this device paired with, as one small file in its home.
pub struct Paired {
    dir: PathBuf,
}

impl Paired {
    /// The list in the home at `dir`. A missing file is an empty list.
    pub fn open(dir: &Path) -> Self {
        Self {
            dir: dir.to_path_buf(),
        }
    }

    /// The daemons this device paired with, in the order they were added.
    pub fn list(&self) -> Result<Vec<PairedDaemon>, Error> {
        let stored = read(&self.dir)?;
        daemons(&self.dir, stored)
    }

    /// Finds a daemon by its name, or by a prefix of its endpoint id. A name matches exactly; an
    /// id prefix matches the lower-case hex. More than one match is refused.
    pub fn find(&self, name: &str) -> Result<PairedDaemon, Error> {
        let daemons = self.list()?;
        if let Some(found) = daemons.iter().find(|daemon| daemon.name == name) {
            return Ok(found.clone());
        }
        let wanted = name.to_ascii_lowercase();
        if wanted.is_empty() || !wanted.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(Error::Policy(format!(
                "no paired daemon is called {name}; `sluice pair --list` shows them"
            )));
        }
        let mut matches = daemons
            .iter()
            .filter(|daemon| hex(&daemon.endpoint_id).starts_with(&wanted));
        match (matches.next(), matches.next()) {
            (Some(found), None) => Ok(found.clone()),
            (Some(_), Some(_)) => Err(Error::Policy(format!(
                "{name} names more than one paired daemon; use the whole endpoint id"
            ))),
            _ => Err(Error::Policy(format!(
                "no paired daemon is called {name}; `sluice pair --list` shows them"
            ))),
        }
    }

    /// Writes `daemon` into the list, replacing the entry with its endpoint id. The name is checked
    /// as a device label is.
    pub fn add(&self, daemon: PairedDaemon) -> Result<(), Error> {
        let name = check_label(&daemon.name)?;
        std::fs::create_dir_all(&self.dir)
            .map_err(|e| Error::Storage(format!("cannot make {}: {e}", self.dir.display())))?;
        let mut stored = read(&self.dir)?;
        stored
            .daemons
            .retain(|entry| entry.endpoint_id.to_ascii_lowercase() != hex(&daemon.endpoint_id));
        stored.daemons.push(StoredDaemon {
            name,
            endpoint_id: hex(&daemon.endpoint_id),
            addrs: daemon.addrs.iter().map(ToString::to_string).collect(),
            relay: daemon.relay,
            grant: daemon.grant,
            paired_ms: daemon.paired_ms,
        });
        write(&self.dir, &stored)
    }
}

impl std::fmt::Debug for Paired {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Paired").finish_non_exhaustive()
    }
}

/// The file's contents, or an empty list when it is not there. A file that cannot be parsed is an
/// error and is never rewritten by [`Paired::add`], so a person can fix it.
fn read(dir: &Path) -> Result<Stored, Error> {
    let path = dir.join(FILE);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Stored {
                version: VERSION,
                daemons: Vec::new(),
            });
        }
        Err(e) => return Err(refuse(dir, &e)),
    };
    let stored: Stored = serde_json::from_slice(&bytes).map_err(|e| {
        Error::Storage(format!(
            "{} is not a list of paired daemons: {e}",
            path.display()
        ))
    })?;
    if stored.version != VERSION {
        return Err(Error::Storage(format!(
            "{} is version {} and this build reads version {VERSION}",
            path.display(),
            stored.version
        )));
    }
    Ok(stored)
}

/// The entries as values a caller can use, refusing a broken id or address.
fn daemons(dir: &Path, stored: Stored) -> Result<Vec<PairedDaemon>, Error> {
    let path = dir.join(FILE);
    let mut out = Vec::with_capacity(stored.daemons.len());
    for entry in stored.daemons {
        let endpoint_id = id_from_hex(&entry.endpoint_id).ok_or_else(|| {
            Error::Storage(format!("{} has a broken endpoint id", path.display()))
        })?;
        let mut addrs = Vec::with_capacity(entry.addrs.len());
        for text in entry.addrs {
            addrs.push(SocketAddr::from_str(&text).map_err(|_| {
                Error::Storage(format!("{} has an address that is not one", path.display()))
            })?);
        }
        out.push(PairedDaemon {
            name: entry.name,
            endpoint_id,
            addrs,
            relay: entry.relay,
            grant: entry.grant,
            paired_ms: entry.paired_ms,
        });
    }
    Ok(out)
}

/// Writes the list through the home's private atomic writer, so a crash leaves the old list or the
/// new one.
fn write(dir: &Path, stored: &Stored) -> Result<(), Error> {
    let bytes = serde_json::to_vec_pretty(stored)
        .map_err(|e| Error::Storage(format!("cannot write the paired daemons: {e}")))?;
    write_atomic(dir, FILE, &bytes)
}

fn refuse(dir: &Path, error: &std::io::Error) -> Error {
    Error::Storage(format!("cannot read {}: {error}", dir.join(FILE).display()))
}

/// The 64 lower-case hex characters of an endpoint id.
pub fn hex(bytes: &[u8; 32]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The bytes a 64-character hex string spells.
pub fn id_from_hex(text: &str) -> Option<[u8; 32]> {
    if text.len() != 64 {
        return None;
    }
    let mut id = [0u8; 32];
    for (at, byte) in id.iter_mut().enumerate() {
        *byte = u8::from_str_radix(text.get(at * 2..at * 2 + 2)?, 16).ok()?;
    }
    Some(id)
}

#[cfg(test)]
mod tests {
    use super::{Paired, PairedDaemon, hex, id_from_hex};
    use crate::device::Grant;

    fn daemon(id: [u8; 32], name: &str) -> PairedDaemon {
        PairedDaemon {
            name: name.to_string(),
            endpoint_id: id,
            addrs: Vec::new(),
            relay: None,
            grant: Grant::Deliver,
            paired_ms: 7,
        }
    }

    #[test]
    fn a_daemon_is_found_by_name_and_by_id_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let paired = Paired::open(dir.path());
        paired.add(daemon([0xab; 32], "home-server")).unwrap();
        assert_eq!(paired.find("home-server").unwrap().endpoint_id, [0xab; 32]);
        assert_eq!(paired.find("ababab").unwrap().endpoint_id, [0xab; 32]);
        assert!(paired.find("ABAB").is_ok(), "hex is not case sensitive");
        assert!(paired.find("nas").is_err());
        assert!(paired.find("").is_err());
    }

    #[test]
    fn adding_the_same_daemon_twice_keeps_one_entry_and_the_new_name() {
        let dir = tempfile::tempdir().unwrap();
        let paired = Paired::open(dir.path());
        paired.add(daemon([1; 32], "first")).unwrap();
        paired.add(daemon([1; 32], "second")).unwrap();
        paired.add(daemon([2; 32], "other")).unwrap();
        let list = paired.list().unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].name, "second");
        assert_eq!(list[1].name, "other");
    }

    #[test]
    fn a_file_that_cannot_be_read_is_an_error_and_is_not_rewritten() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("paired.json"), b"not json").unwrap();
        let paired = Paired::open(dir.path());
        assert!(paired.list().is_err());
        assert!(paired.add(daemon([3; 32], "nas")).is_err());
        assert_eq!(
            std::fs::read(dir.path().join("paired.json")).unwrap(),
            b"not json"
        );
    }

    #[test]
    fn the_hex_helpers_agree() {
        assert_eq!(id_from_hex(&hex(&[0x0f; 32])), Some([0x0f; 32]));
        assert_eq!(id_from_hex("abc"), None);
        assert_eq!(hex(&[0; 32]), "0".repeat(64));
    }
}
