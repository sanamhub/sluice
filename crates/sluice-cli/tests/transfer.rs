//! Two real `sluice` processes on loopback: the sender prints a ticket and the receiver writes the
//! file. No relay and no internet are involved.

mod common;

use std::process::Command;

use common::{EXE, data, recv, recv_in, start, start_in, wait_for_sender};

#[test]
fn one_file_crosses_two_processes_with_its_name() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let out = dir.path().join("out");
    let file = dir.path().join("notes.bin");
    let body = data(300 * 1024, 7);
    std::fs::write(&file, &body).unwrap();

    let (mut sender, ticket) = start(&home, &file, "1h");
    let result = recv(&home, &ticket, &out, false);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(std::fs::read(out.join("notes.bin")).unwrap(), body);
    wait_for_sender(&mut sender);
}

/// Both sides are given relative paths and the receiver is given no `--out`, so it writes into the
/// folder it was started from. `iroh-blobs` refuses a relative path, so this fails without the
/// resolution both commands do first.
#[test]
fn a_relative_file_and_no_out_still_work() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let source = dir.path().join("source");
    let landing = dir.path().join("landing");
    std::fs::create_dir(&source).unwrap();
    std::fs::create_dir(&landing).unwrap();
    let body = data(64 * 1024, 3);
    std::fs::write(source.join("relative.bin"), &body).unwrap();

    let (mut sender, ticket) = start_in(&source, &home, &["relative.bin", "--expires", "1h"]);
    let result = recv_in(&landing, &home, &ticket, &[]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(std::fs::read(landing.join("relative.bin")).unwrap(), body);
    wait_for_sender(&mut sender);
}

#[test]
fn a_restarted_sender_prints_the_same_ticket() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let file = dir.path().join("same.bin");
    std::fs::write(&file, data(1024, 5)).unwrap();

    let (sender, first) = start(&home, &file, "never");
    drop(sender);
    let (_sender, second) = start(&home, &file, "never");
    let first = sluice_core::ticket::Ticket::decode(&first).unwrap();
    let second = sluice_core::ticket::Ticket::decode(&second).unwrap();
    assert_eq!(first.endpoint_id(), second.endpoint_id());
    assert_eq!(first.body(), second.body());
    assert_eq!(first.expiry_ms(), second.expiry_ms());
    assert_eq!(first.relay_url(), second.relay_url());
    assert!(
        first
            .addrs()
            .iter()
            .any(|addr| second.addrs().contains(addr)),
        "no address survived the restart"
    );
}

#[test]
fn a_folder_crosses_two_processes_inside_its_own_name() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let out = dir.path().join("out");
    let tree = dir.path().join("tree");
    std::fs::create_dir_all(tree.join("sub")).unwrap();
    let big = data(1200 * 1024, 3);
    std::fs::write(tree.join("big.bin"), &big).unwrap();
    std::fs::write(tree.join("sub").join("note.txt"), b"nested").unwrap();
    std::fs::write(tree.join("small.txt"), b"small").unwrap();

    let (mut sender, ticket) = start(&home, &tree, "1h");
    let result = recv(&home, &ticket, &out, false);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let inside = out.join("tree");
    assert_eq!(std::fs::read(inside.join("big.bin")).unwrap(), big);
    assert_eq!(
        std::fs::read(inside.join("sub").join("note.txt")).unwrap(),
        b"nested"
    );
    assert_eq!(std::fs::read(inside.join("small.txt")).unwrap(), b"small");
    wait_for_sender(&mut sender);
}

#[test]
fn a_name_clash_refuses_before_writing_and_force_replaces() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let out = dir.path().join("out");
    std::fs::create_dir(&out).unwrap();
    let file = dir.path().join("a.txt");
    std::fs::write(&file, b"new body").unwrap();
    std::fs::write(out.join("a.txt"), b"old body").unwrap();

    let (mut sender, ticket) = start(&home, &file, "1h");
    let refused = recv(&home, &ticket, &out, false);
    assert_eq!(refused.status.code(), Some(6));
    assert!(String::from_utf8_lossy(&refused.stderr).contains("--force"));
    assert_eq!(std::fs::read(out.join("a.txt")).unwrap(), b"old body");

    let replaced = recv(&home, &ticket, &out, true);
    assert!(
        replaced.status.success(),
        "{}",
        String::from_utf8_lossy(&replaced.stderr)
    );
    assert_eq!(std::fs::read(out.join("a.txt")).unwrap(), b"new body");
    wait_for_sender(&mut sender);
}

#[test]
fn a_hidden_top_level_name_needs_its_flag() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let out = dir.path().join("out");
    let file = dir.path().join(".profile");
    std::fs::write(&file, b"export PATH").unwrap();

    let (mut sender, ticket) = start(&home, &file, "1h");
    let refused = recv(&home, &ticket, &out, false);
    assert_eq!(refused.status.code(), Some(6));
    let text = String::from_utf8_lossy(&refused.stderr);
    assert!(text.contains("--allow-hidden"), "{text}");
    assert!(text.contains(".profile"), "{text}");
    assert!(!out.join(".profile").exists());

    let accepted = recv_in(
        out.parent().unwrap(),
        &home,
        &ticket,
        &["--out", out.to_str().unwrap(), "--allow-hidden"],
    );
    assert!(
        accepted.status.success(),
        "{}",
        String::from_utf8_lossy(&accepted.stderr)
    );
    assert_eq!(std::fs::read(out.join(".profile")).unwrap(), b"export PATH");
    wait_for_sender(&mut sender);
}

#[cfg(unix)]
#[test]
fn a_top_level_link_is_replaced_and_never_followed() {
    use std::os::unix::fs::symlink;

    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let out = dir.path().join("out");
    let victim = dir.path().join("victim.txt");
    std::fs::create_dir(&out).unwrap();
    std::fs::write(&victim, b"must not change").unwrap();
    let file = dir.path().join("replacement.txt");
    std::fs::write(&file, b"new content").unwrap();
    // The sender names the file `link`, so the receiver's target is `out/link`, a link to a file
    // outside the output folder.
    let source = dir.path().join("source");
    std::fs::create_dir(&source).unwrap();
    std::fs::write(source.join("link"), b"new content").unwrap();
    symlink(&victim, out.join("link")).unwrap();

    let (mut sender, ticket) = start_in(&source, &home, &["link", "--expires", "1h"]);
    let refused = recv(&home, &ticket, &out, false);
    assert_eq!(refused.status.code(), Some(6));

    let replaced = recv(&home, &ticket, &out, true);
    assert!(
        replaced.status.success(),
        "{}",
        String::from_utf8_lossy(&replaced.stderr)
    );
    assert_eq!(std::fs::read(out.join("link")).unwrap(), b"new content");
    assert_eq!(std::fs::read(&victim).unwrap(), b"must not change");
    wait_for_sender(&mut sender);
}

#[test]
fn an_expired_ticket_is_refused_and_the_sender_stops() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let out = dir.path().join("out");
    let file = dir.path().join("short.txt");
    std::fs::write(&file, b"short lived").unwrap();

    let (mut sender, ticket) = start(&home, &file, "1s");
    // The sender serves until its own deadline passes, which is the clock the test waits on.
    wait_for_sender(&mut sender);
    let refused = recv(&home, &ticket, &out, false);
    assert_eq!(refused.status.code(), Some(6));
    assert!(String::from_utf8_lossy(&refused.stderr).contains("expired"));
}

#[test]
fn the_sender_refuses_a_second_send_from_one_home() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let first = dir.path().join("first.bin");
    let second = dir.path().join("second.bin");
    std::fs::write(&first, data(1024, 1)).unwrap();
    std::fs::write(&second, data(1024, 2)).unwrap();
    let (_sender, _ticket) = start(&home, &first, "1h");
    let refused = Command::new(EXE)
        .env("SLUICE_HOME", &home)
        .args(["send", second.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(refused.status.code(), Some(6));
    let text = String::from_utf8_lossy(&refused.stderr);
    assert!(text.contains("send both in one command instead"), "{text}");
}

#[test]
fn two_arguments_with_one_final_name_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let one = dir.path().join("one");
    let two = dir.path().join("two");
    std::fs::create_dir(&one).unwrap();
    std::fs::create_dir(&two).unwrap();
    std::fs::write(one.join("x"), b"one").unwrap();
    std::fs::write(two.join("x"), b"two").unwrap();

    let result = Command::new(EXE)
        .env("SLUICE_HOME", &home)
        .args([
            "send",
            one.join("x").to_str().unwrap(),
            two.join("x").to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(6));
    let text = String::from_utf8_lossy(&result.stderr);
    assert!(text.contains("would both arrive as x"), "{text}");
}

#[test]
fn a_chosen_port_is_the_one_in_the_ticket() {
    let probe = std::net::UdpSocket::bind("0.0.0.0:0").unwrap();
    let port = probe.local_addr().unwrap().port();
    drop(probe);

    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let file = dir.path().join("chosen.bin");
    std::fs::write(&file, data(1024, 1)).unwrap();
    let port_text = port.to_string();
    let (_sender, ticket) = start_in(
        dir.path(),
        &home,
        &["chosen.bin", "--expires", "1h", "--port", &port_text],
    );
    let decoded = sluice_core::ticket::Ticket::decode(&ticket).unwrap();
    assert!(
        decoded.addrs().iter().any(|addr| addr.port() == port),
        "the ticket names {:?}, not port {port}",
        decoded.addrs()
    );
}

#[test]
fn a_taken_port_is_refused_with_its_number() {
    let probe = std::net::UdpSocket::bind("0.0.0.0:0").unwrap();
    let port = probe.local_addr().unwrap().port();

    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let file = dir.path().join("taken.bin");
    std::fs::write(&file, data(1024, 1)).unwrap();
    let port_text = port.to_string();
    let result = Command::new(EXE)
        .env("SLUICE_HOME", &home)
        .args(["send", file.to_str().unwrap(), "--port", &port_text])
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(5));
    let text = String::from_utf8_lossy(&result.stderr);
    assert!(
        text.contains(&format!("cannot bind UDP port {port}")),
        "{text}"
    );
}
