//! A paired device: the grant it holds, the file the grants live in, and the rules of `SPEC.md`
//! section 11.1.
//!
//! The file holds no secret. It is a list of endpoint ids, labels and grants, and losing it loses
//! the record rather than a key: a device pairs again with a new ticket. A file the daemon cannot
//! read is no grants at all, so a corrupt file refuses every peer instead of admitting one.

use std::{
    collections::BTreeMap,
    fmt,
    path::{Path, PathBuf},
    str::FromStr,
    sync::{Arc, Mutex},
};

use serde::{Deserialize, Serialize};

use crate::Error;
use crate::home::atomic::write_atomic;

/// The file in the home that holds the grants.
const FILE: &str = "devices.json";

/// The version of that file this release writes and reads.
const VERSION: u8 = 1;

/// The longest label `devices rename` accepts, in characters. A label is shown in a list and never
/// used to find a device, so the bound is for the person reading it.
pub const MAX_LABEL: usize = 64;

/// What a paired device may do, as `SPEC.md` section 11.1 defines it. `control` includes what
/// `deliver` allows.
///
/// # Examples
///
/// ```
/// use sluice_core::device::Grant;
///
/// assert!(Grant::Control.includes(Grant::Deliver));
/// assert!(!Grant::Deliver.includes(Grant::Control));
/// assert_eq!(Grant::from_wire(0), Some(Grant::Deliver));
/// assert_eq!(Grant::from_wire(2), None);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Grant {
    /// Hand the daemon a ticket and have it received into the inbox. The default.
    Deliver,
    /// Every call of the control interface, which includes what `deliver` allows.
    Control,
}

impl Grant {
    /// The byte a pairing ticket carries for this grant. `SPEC.md` section 4 gives the values.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::device::Grant;
    ///
    /// assert_eq!(Grant::Deliver.wire(), 0);
    /// assert_eq!(Grant::Control.wire(), 1);
    /// ```
    pub fn wire(self) -> u8 {
        match self {
            Self::Deliver => 0,
            Self::Control => 1,
        }
    }

    /// The grant a pairing ticket's byte names, or `None` for a byte no grant uses.
    pub fn from_wire(byte: u8) -> Option<Self> {
        match byte {
            0 => Some(Self::Deliver),
            1 => Some(Self::Control),
            _ => None,
        }
    }

    /// Whether this grant allows what `wanted` allows.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::device::Grant;
    ///
    /// assert!(Grant::Deliver.includes(Grant::Deliver));
    /// assert!(Grant::Control.includes(Grant::Deliver));
    /// assert!(!Grant::Deliver.includes(Grant::Control));
    /// ```
    pub fn includes(self, wanted: Self) -> bool {
        matches!(
            (self, wanted),
            (Self::Control, _) | (Self::Deliver, Self::Deliver)
        )
    }

    /// The word a list and a pairing command print.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Deliver => "deliver",
            Self::Control => "control",
        }
    }
}

impl fmt::Display for Grant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Grant {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text {
            "deliver" => Ok(Self::Deliver),
            "control" => Ok(Self::Control),
            other => Err(format!(
                "{other} is not a grant; the grants are deliver and control"
            )),
        }
    }
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
    /// `grant`. The file is written before this returns, so a pairing that answered success is on
    /// disk.
    pub fn grant_to(
        &self,
        endpoint_id: [u8; 32],
        grant: Grant,
        paired_ms: u64,
    ) -> Result<Device, Error> {
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
            label: default_label(&endpoint_id),
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
    /// devices.grant_to([2; 32], Grant::Deliver, 1)?;
    /// assert_eq!(devices.rename(&[2; 32], "laptop")?.label, "laptop");
    /// # Ok::<(), sluice_core::Error>(())
    /// ```
    pub fn rename(&self, endpoint_id: &[u8; 32], label: &str) -> Result<Device, Error> {
        let label = label.trim();
        if label.is_empty() || label.chars().count() > MAX_LABEL {
            return Err(Error::Policy(format!(
                "a label is 1 to {MAX_LABEL} characters"
            )));
        }
        if label.chars().any(char::is_control) {
            return Err(Error::Policy("a label has no control characters".into()));
        }
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
        let renamed = Device {
            label: label.to_string(),
            ..device
        };
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
mod tests {
    use std::fs;

    use super::{Devices, Grant, MAX_LABEL, default_label};

    /// A folder of its own for one test, removed when it goes.
    struct Dir(std::path::PathBuf);

    impl Dir {
        fn new(name: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("sluice-device-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).expect("a folder");
            Self(path)
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// A label starts as eight hex characters and a rename changes only the label.
    #[test]
    fn a_grant_is_written_with_its_default_label_and_survives_a_reopen() {
        let dir = Dir::new("label");
        let devices = Devices::open(&dir.0);
        let device = devices.grant_to([0xab; 32], Grant::Deliver, 7).unwrap();
        assert_eq!(device.label, "abababab");
        assert_eq!(default_label(&[0xab; 32]), device.label);

        let reopened = Devices::open(&dir.0);
        assert_eq!(reopened.grant(&[0xab; 32]), Some(Grant::Deliver));
        assert_eq!(reopened.list().unwrap()[0].paired_ms, 7);

        let renamed = reopened.rename(&[0xab; 32], "  laptop ").unwrap();
        assert_eq!(renamed.label, "laptop");
        assert_eq!(renamed.grant, Grant::Deliver);
        assert_eq!(Devices::open(&dir.0).list().unwrap()[0].label, "laptop");
    }

    /// A second redemption keeps the grant it has, and a wider one widens it, without a second
    /// entry.
    #[test]
    fn a_second_grant_keeps_one_entry_and_never_narrows_it() {
        let dir = Dir::new("again");
        let devices = Devices::open(&dir.0);
        let first = devices.grant_to([3; 32], Grant::Deliver, 1).unwrap();
        let again = devices.grant_to([3; 32], Grant::Deliver, 2).unwrap();
        assert_eq!(again, first);
        assert_eq!(devices.list().unwrap().len(), 1);

        let wider = devices.grant_to([3; 32], Grant::Control, 3).unwrap();
        assert_eq!(wider.grant, Grant::Control);
        assert!(devices.allows(&[3; 32], Grant::Deliver));
        assert!(devices.allows(&[3; 32], Grant::Control));
    }

    /// A revoke removes the grant, and a file that cannot be read admits nobody and is left as it
    /// is.
    #[test]
    fn a_revoke_forgets_the_grant_and_a_broken_file_admits_nobody() {
        let dir = Dir::new("revoke");
        let devices = Devices::open(&dir.0);
        devices.grant_to([4; 32], Grant::Deliver, 1).unwrap();
        assert!(devices.revoke(&[4; 32]).unwrap().is_some());
        assert!(!devices.allows(&[4; 32], Grant::Deliver));
        assert!(devices.revoke(&[4; 32]).unwrap().is_none());

        let broken = Dir::new("broken");
        let path = broken.0.join("devices.json");
        fs::write(&path, b"not json").unwrap();
        let devices = Devices::open(&broken.0);
        assert!(devices.is_broken());
        assert!(!devices.allows(&[5; 32], Grant::Deliver));
        assert!(devices.list().is_err());
        assert!(devices.grant_to([5; 32], Grant::Deliver, 1).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"not json");
    }

    /// A label with a control character or over the bound is refused before anything is written.
    #[test]
    fn a_label_is_one_line_and_bounded() {
        let dir = Dir::new("labels");
        let devices = Devices::open(&dir.0);
        devices.grant_to([6; 32], Grant::Deliver, 1).unwrap();
        assert!(devices.rename(&[6; 32], "two\nlines").is_err());
        assert!(
            devices
                .rename(&[6; 32], &"x".repeat(MAX_LABEL + 1))
                .is_err()
        );
        assert!(devices.rename(&[6; 32], "  ").is_err());
        assert_eq!(devices.list().unwrap()[0].label, "06060606");
    }

    /// A duplicate id in a hand-edited file is a file the daemon cannot read, not one that wins a
    /// race.
    #[test]
    fn a_duplicate_endpoint_id_is_a_broken_file() {
        let dir = Dir::new("duplicate");
        let id = "07".repeat(32);
        let text = format!(
            r#"{{"version":1,"devices":[
                {{"endpoint_id":"{id}","label":"a","grant":"deliver","paired_ms":1}},
                {{"endpoint_id":"{id}","label":"b","grant":"control","paired_ms":2}}]}}"#
        );
        fs::write(dir.0.join("devices.json"), text).unwrap();
        assert!(Devices::open(&dir.0).is_broken());
    }
}
