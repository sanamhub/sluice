//! `--rendezvous`: the relay half off and written so, and nothing that only a relay reads.

use crate::{Mode, check_config};

const OFF: &str = "enable_relay = false\nenable_metrics = false\n";
const TLS: &str = "[tls]\nquic_bind_addr = \"0.0.0.0:7842\"\ncert_mode = \"Manual\"\n";

fn refused(text: &str, mode: Mode, says: &str) {
    let message = check_config(text, mode).expect_err("refused").to_string();
    assert!(message.contains(says), "{text:?} {mode:?}: {message}");
    assert!(!message.contains('\n'), "one line: {message}");
}

#[test]
fn the_shipped_rendezvous_config_starts_only_as_rendezvous() {
    let manual = super::read(&super::rendezvous_dir().join("manual.toml"));
    assert_eq!(check_config(&manual, Mode::Rendezvous), Ok(()));
    refused(&manual, Mode::Closed, "start it with --rendezvous");
    refused(&manual, Mode::Open, "start it with --rendezvous");
}

#[test]
fn enable_relay_must_be_written_false() {
    let quic = format!("enable_metrics = false\nenable_quic_addr_discovery = true\n{TLS}");
    refused(&quic, Mode::Rendezvous, "needs `enable_relay = false`");
    let on = format!("enable_relay = true\n{quic}");
    refused(&on, Mode::Rendezvous, "needs `enable_relay = false`");
    let off = format!("enable_relay = false\n{quic}");
    assert_eq!(check_config(&off, Mode::Rendezvous), Ok(()));
}

#[test]
fn access_and_limits_are_refused() {
    refused(
        &format!("{OFF}access = \"everyone\"\n"),
        Mode::Rendezvous,
        "`access` is set",
    );
    refused(
        &format!("{OFF}[access]\nallowlist = []\n"),
        Mode::Rendezvous,
        "`access` is set",
    );
    refused(
        &format!("{OFF}[limits.client.rx]\nbytes_per_second = 1\n"),
        Mode::Rendezvous,
        "[limits] is set",
    );
    refused(
        &format!("{OFF}[limits]\n"),
        Mode::Rendezvous,
        "[limits] is set",
    );
}

#[test]
fn every_other_rule_holds_in_rendezvous_mode() {
    refused(
        "enable_relay = false\n",
        Mode::Rendezvous,
        "`enable_metrics` is not set",
    );
    refused(
        &format!("{OFF}{TLS}dangerous_http_only = true\n"),
        Mode::Rendezvous,
        "`dangerous_http_only` is true",
    );
    refused(
        &format!("{OFF}enable_relays = false\n"),
        Mode::Rendezvous,
        "line 3: unknown key `enable_relays`",
    );
    refused(
        &format!("{OFF}[tls]\ncert_dri = \"/c\"\n"),
        Mode::Rendezvous,
        "docker/rendezvous/README.md",
    );
}
