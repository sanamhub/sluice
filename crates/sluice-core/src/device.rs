//! A paired device: the grant it holds, the file the grants live in, and the rules of `SPEC.md`
//! section 11.1.
//!
//! The file holds no secret. It is a list of endpoint ids, labels and grants, and losing it loses
//! the record rather than a key: a device pairs again with a new ticket. A file the daemon cannot
//! read is no grants at all, so a corrupt file refuses every peer instead of admitting one.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use serde::{Deserialize, Serialize};

use crate::Error;
use crate::home::atomic::write_atomic;

/// The file in the home that holds the grants.
const FILE: &str = "devices.json";

/// The version of that file this release writes and reads.
const VERSION: u8 = 1;

mod grant;
pub use grant::Grant;

/// The longest label `devices rename` accepts, in characters. A label is shown in a list and never
/// used to find a device, so the bound is for the person reading it.
pub const MAX_LABEL: usize = 64;

/// The label as it is stored: trimmed, 1 to [`MAX_LABEL`] characters, with no control character. A
/// pairing ticket may name the device's label when it is made, and a rename may change it later.
pub fn check_label(label: &str) -> Result<String, Error> {
    let label = label.trim();
    if label.is_empty() || label.chars().count() > MAX_LABEL {
        return Err(Error::Policy(format!(
            "a label is 1 to {MAX_LABEL} characters"
        )));
    }
    if label.chars().any(char::is_control) {
        return Err(Error::Policy("a label has no control characters".into()));
    }
    Ok(label.to_string())
}

/// One device the daemon has a grant for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    /// The endpoint id the device authenticated with when it paired.
    pub endpoint_id: [u8; 32],
    /// What a person calls it. The first 8 hex characters of the id until someone renames it.
    pub label: String,
    /// What it may do.
    pub grant: Grant,
    /// When it paired, in UTC milliseconds since the Unix epoch.
    pub paired_ms: u64,
}

impl Device {
    /// The label a device starts with: the first 8 hex characters of its endpoint id.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::device::default_label;
    ///
    /// assert_eq!(default_label(&[0xab; 32]), "abababab");
    /// ```
    pub fn default_label(endpoint_id: &[u8; 32]) -> String {
        default_label(endpoint_id)
    }
}

/// The label a device starts with, before anyone renames it.
pub fn default_label(endpoint_id: &[u8; 32]) -> String {
    endpoint_id[..4]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The grants one daemon has written, held in memory and in `$SLUICE_HOME/devices.json`.
///
/// A file that cannot be read leaves every method that admits a peer returning nothing, and every
/// method that writes returning an error, so the daemon fails closed and leaves the file alone for
/// its user to fix.
#[derive(Clone)]
pub struct Devices {
    dir: PathBuf,
    state: Arc<Mutex<State>>,
}

#[derive(Default)]
struct State {
    /// The file could not be read, so it is not rewritten and no grant in it is honoured.
    broken: bool,
    devices: BTreeMap<[u8; 32], Device>,
}

/// The file as it is written.
#[derive(Serialize, Deserialize)]
struct Stored {
    version: u8,
    devices: Vec<StoredDevice>,
}

#[derive(Serialize, Deserialize)]
struct StoredDevice {
    endpoint_id: String,
    label: String,
    grant: Grant,
    paired_ms: u64,
}

impl Devices {
    /// Reads the grants of the home at `dir`. A missing file is no grants; a file that cannot be
    /// read leaves the daemon refusing every peer.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::device::Devices;
    ///
    /// let dir = std::env::temp_dir().join(format!("sluice-devices-{}", std::process::id()));
    /// let devices = Devices::open(&dir);
    /// assert!(devices.list()?.is_empty());
    /// # Ok::<(), sluice_core::Error>(())
    /// ```
    pub fn open(dir: &Path) -> Self {
        let (broken, devices) = match read(dir) {
            Ok(devices) => (false, devices),
            Err(_) => (true, BTreeMap::new()),
        };
        Self {
            dir: dir.to_path_buf(),
            state: Arc::new(Mutex::new(State { broken, devices })),
        }
    }

    /// Whether the file could not be read. A daemon that finds this refuses to pair and reports the
    /// failure to `ListDevices` rather than an empty list.
    pub fn is_broken(&self) -> bool {
        self.lock().broken
    }

    /// The grant `endpoint_id` holds, if any. A file the daemon cannot read holds none.
    pub fn grant(&self, endpoint_id: &[u8; 32]) -> Option<Grant> {
        let state = self.lock();
        if state.broken {
            return None;
        }
        state.devices.get(endpoint_id).map(|device| device.grant)
    }

    /// Whether `endpoint_id` may do what `wanted` allows.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::device::{Devices, Grant};
    ///
    /// let dir = std::env::temp_dir().join(format!("sluice-grant-{}", std::process::id()));
    /// let devices = Devices::open(&dir);
    /// assert!(!devices.allows(&[1; 32], Grant::Deliver));
    /// # Ok::<(), sluice_core::Error>(())
    /// ```
    pub fn allows(&self, endpoint_id: &[u8; 32], wanted: Grant) -> bool {
        self.grant(endpoint_id)
            .is_some_and(|grant| grant.includes(wanted))
    }

    /// Every device, in endpoint id order. Fails when the file could not be read, so a caller
    /// shows the failure rather than an empty list.
    pub fn list(&self) -> Result<Vec<Device>, Error> {
        let state = self.lock();
        if state.broken {
            return Err(unreadable(&self.dir));
        }
        Ok(state.devices.values().cloned().collect())
    }

    /// Writes a grant for `endpoint_id`, or keeps the one it has when that grant already includes
    /// `grant`. `label` names a device that is new to the list, and a device that is already there
    /// keeps the label it has. The file is written before this returns, so a pairing that answered
    /// success is on disk.
    pub fn grant_to(
        &self,
        endpoint_id: [u8; 32],
        grant: Grant,
        paired_ms: u64,
        label: Option<&str>,
    ) -> Result<Device, Error> {
        let label = match label {
            Some(label) => Some(check_label(label)?),
            None => None,
        };
        let mut state = self.lock();
        if state.broken {
            return Err(unreadable(&self.dir));
        }
        if let Some(device) = state.devices.get(&endpoint_id)
            && device.grant.includes(grant)
        {
            return Ok(device.clone());
        }
        let device = Device {
            endpoint_id,
            label: label.unwrap_or_else(|| default_label(&endpoint_id)),
            grant,
            paired_ms,
        };
        let mut next = state.devices.clone();
        next.insert(endpoint_id, device.clone());
        self.save(&next)?;
        state.devices = next;
        Ok(device)
    }

    /// Changes a device's label and nothing else.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::device::{Devices, Grant};
    ///
    /// let dir = std::env::temp_dir().join(format!("sluice-rename-{}", std::process::id()));
    /// std::fs::create_dir_all(&dir)?;
    /// let devices = Devices::open(&dir);
    /// devices.grant_to([2; 32], Grant::Deliver, 1, None)?;
    /// assert_eq!(devices.rename(&[2; 32], "laptop")?.label, "laptop");
    /// # Ok::<(), sluice_core::Error>(())
    /// ```
    pub fn rename(&self, endpoint_id: &[u8; 32], label: &str) -> Result<Device, Error> {
        let label = check_label(label)?;
        let mut state = self.lock();
        if state.broken {
            return Err(unreadable(&self.dir));
        }
        let Some(device) = state.devices.get(endpoint_id).cloned() else {
            return Err(Error::Policy(format!(
                "no device has the endpoint id {}",
                default_label(endpoint_id)
            )));
        };
        let renamed = Device { label, ..device };
        let mut next = state.devices.clone();
        next.insert(*endpoint_id, renamed.clone());
        self.save(&next)?;
        state.devices = next;
        Ok(renamed)
    }

    /// Removes a grant and answers the device that held it, so the caller can close its
    /// connections. `None` means there was no grant for that id.
    pub fn revoke(&self, endpoint_id: &[u8; 32]) -> Result<Option<Device>, Error> {
        let mut state = self.lock();
        if state.broken {
            return Err(unreadable(&self.dir));
        }
        let Some(device) = state.devices.get(endpoint_id).cloned() else {
            return Ok(None);
        };
        let mut next = state.devices.clone();
        next.remove(endpoint_id);
        self.save(&next)?;
        state.devices = next;
        Ok(Some(device))
    }

    /// Writes `state` to the file. The caller holds the lock and passes the map it will adopt only
    /// if this returns `Ok`, so a failed write changes nothing.
    fn save(&self, devices: &BTreeMap<[u8; 32], Device>) -> Result<(), Error> {
        let stored = Stored {
            version: VERSION,
            devices: devices
                .values()
                .map(|device| StoredDevice {
                    endpoint_id: hex(&device.endpoint_id),
                    label: device.label.clone(),
                    grant: device.grant,
                    paired_ms: device.paired_ms,
                })
                .collect(),
        };
        let bytes = serde_json::to_vec_pretty(&stored)
            .map_err(|e| Error::Storage(format!("cannot write {FILE}: {e}")))?;
        write_atomic(&self.dir, FILE, &bytes)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn unreadable(dir: &Path) -> Error {
    Error::Storage(format!(
        "{} could not be read, so no device is paired until it is fixed",
        dir.join(FILE).display()
    ))
}

/// The grants in `dir`, or an error for a file that exists and cannot be read. A missing file is
/// no grants.
fn read(dir: &Path) -> Result<BTreeMap<[u8; 32], Device>, Error> {
    let path = dir.join(FILE);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(e) => return Err(Error::Storage(format!("cannot read {FILE}: {e}"))),
    };
    let stored: Stored = serde_json::from_slice(&bytes)
        .map_err(|e| Error::Storage(format!("{FILE} is not a grants file: {e}")))?;
    if stored.version != VERSION {
        return Err(Error::Storage(format!(
            "{FILE} has version {}, and this release reads {VERSION}",
            stored.version
        )));
    }
    let mut devices = BTreeMap::new();
    for device in stored.devices {
        let endpoint_id = unhex(&device.endpoint_id)?;
        if devices.contains_key(&endpoint_id) {
            return Err(Error::Storage(format!(
                "{FILE} lists the endpoint id {} twice",
                default_label(&endpoint_id)
            )));
        }
        devices.insert(
            endpoint_id,
            Device {
                endpoint_id,
                label: device.label,
                grant: device.grant,
                paired_ms: device.paired_ms,
            },
        );
    }
    Ok(devices)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn unhex(text: &str) -> Result<[u8; 32], Error> {
    let bad = || {
        Error::Storage(format!(
            "{FILE} holds an endpoint id that is not 64 hex digits"
        ))
    };
    if text.len() != 64 {
        return Err(bad());
    }
    let mut out = [0u8; 32];
    for (byte, pair) in out.iter_mut().zip(text.as_bytes().chunks(2)) {
        let pair = std::str::from_utf8(pair).map_err(|_| bad())?;
        *byte = u8::from_str_radix(pair, 16).map_err(|_| bad())?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
