//! A daemon pairs a device, the device delivers into its inbox, and the lists on both sides say
//! what happened. The daemon is a process of its own, so this is the command line end to end.

use crate::common::daemon::start_with;
use crate::common::{sluice, stdout_of};

#[test]
fn a_device_pairs_delivers_and_is_listed_renamed_and_revoked() {
    let dir = tempfile::tempdir().unwrap();
    let daemon_home = dir.path().join("daemon-home");
    let device_home = dir.path().join("device-home");
    let root = dir.path().join("root");
    std::fs::create_dir_all(&root).unwrap();
    let inbox = root.join("inbox");

    // A daemon serves pairing only when its home has an identity key.
    let who = stdout_of(sluice(&daemon_home).arg("id"));
    assert!(crate::common::is_endpoint_id(who.trim()), "{who:?}");
    let daemon = start_with(
        &daemon_home,
        &[&root],
        "trace",
        &[
            "--inbox",
            inbox.to_str().unwrap(),
            "--inbox-quota",
            "1000000",
        ],
    );

    // The daemon makes one ticket, for one device, with a label.
    let ticket = stdout_of(sluice(&daemon_home).args([
        "pair", "--grant", "deliver", "--ttl", "600", "--name", "laptop",
    ]));
    let ticket = ticket.trim().to_string();
    assert!(ticket.starts_with("sluice1"), "{ticket:?}");

    // The device redeems it and keeps the daemon under a name of its own.
    let paired =
        stdout_of(sluice(&device_home).args(["pair", "--redeem", &ticket, "--name", "home"]));
    assert!(paired.contains("paired with home"), "{paired:?}");
    let mine = stdout_of(sluice(&device_home).args(["pair", "--list"]));
    assert!(mine.contains("home"), "{mine:?}");
    assert!(mine.contains("deliver"), "{mine:?}");

    // The daemon lists the device under the label the ticket named.
    let devices = stdout_of(sluice(&daemon_home).args(["devices", "list"]));
    assert!(devices.contains("laptop"), "{devices:?}");
    assert!(devices.contains("deliver"), "{devices:?}");

    // A named delivery lands under that name in the inbox.
    let file = dir.path().join("note.txt");
    std::fs::write(&file, b"delivered into the inbox").unwrap();
    sluice(&device_home)
        .args([
            "deliver",
            "home",
            file.to_str().unwrap(),
            "--name",
            "report",
        ])
        .assert()
        .success();
    assert_eq!(
        std::fs::read(inbox.join("report").join("note.txt")).unwrap(),
        b"delivered into the inbox"
    );

    // The ticket is secret material and never reached the daemon's log, even at `trace`.
    assert!(
        !daemon.log_text().contains(&ticket),
        "a pairing ticket reached the log"
    );

    // A rename changes the label and nothing else, and a revoke removes the grant.
    sluice(&daemon_home)
        .args(["devices", "rename", "laptop", "work"])
        .assert()
        .success();
    let devices = stdout_of(sluice(&daemon_home).args(["devices", "list"]));
    assert!(devices.contains("work"), "{devices:?}");
    assert!(!devices.contains("laptop"), "{devices:?}");
    sluice(&daemon_home)
        .args(["devices", "revoke", "work"])
        .assert()
        .success();
    let devices = stdout_of(sluice(&daemon_home).args(["devices", "list"]));
    assert!(devices.trim().is_empty(), "{devices:?}");
}
