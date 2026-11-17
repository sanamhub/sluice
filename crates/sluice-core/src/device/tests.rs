//! The grants file: what it holds, how a broken file fails closed, and how a label is kept.

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
    let device = devices
        .grant_to([0xab; 32], Grant::Deliver, 7, None)
        .unwrap();
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
    let first = devices.grant_to([3; 32], Grant::Deliver, 1, None).unwrap();
    let again = devices.grant_to([3; 32], Grant::Deliver, 2, None).unwrap();
    assert_eq!(again, first);
    assert_eq!(devices.list().unwrap().len(), 1);

    let wider = devices.grant_to([3; 32], Grant::Control, 3, None).unwrap();
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
    devices.grant_to([4; 32], Grant::Deliver, 1, None).unwrap();
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
    assert!(devices.grant_to([5; 32], Grant::Deliver, 1, None).is_err());
    assert_eq!(fs::read(&path).unwrap(), b"not json");
}

/// A label with a control character or over the bound is refused before anything is written.
#[test]
fn a_label_is_one_line_and_bounded() {
    let dir = Dir::new("labels");
    let devices = Devices::open(&dir.0);
    devices.grant_to([6; 32], Grant::Deliver, 1, None).unwrap();
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
