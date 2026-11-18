#![allow(clippy::unwrap_used)]
//! The schema the guard checked and this binary read are one schema: every key guard accepts is a
//! key this parser accepts, and every unknown key either refuses. A few values that change the
//! listener are checked too.

use sluice_relay::config::{Config, ConfigError};
use sluice_relay_guard::{Mode, check_config};

const ALL: &str = "enable_relay = true\n\
                   enable_quic_addr_discovery = false\n\
                   enable_metrics = false\n\
                   http_bind_addr = \"127.0.0.1:8080\"\n\
                   metrics_bind_addr = \"127.0.0.1:9090\"\n\
                   key_cache_capacity = 64\n\
                   [access]\nallowlist = []\n\
                   [limits]\naccept_conn_limit = 1.0\naccept_conn_burst = 2\n\
                   [limits.client.rx]\nbytes_per_second = 1024\nmax_burst_bytes = 2048\n\
                   [tls]\nhttps_bind_addr = \"127.0.0.1:8443\"\nquic_bind_addr = \"127.0.0.1:7842\"\n\
                   cert_mode = \"Manual\"\ncert_dir = \"/etc/relay/certs\"\n\
                   manual_cert_path = \"/etc/relay/certs/a.crt\"\n\
                   manual_key_path = \"/etc/relay/certs/a.key\"\nhostname = [\"relay.example\"]\n\
                   contact = \"ops@example\"\nprod_tls = true\n\
                   [meet]\nenable = false\n";

#[test]
fn every_key_the_guard_accepts_is_a_key_this_reader_accepts() {
    assert_eq!(check_config(ALL, Mode::Closed, None, false), Ok(()));
    assert!(Config::parse(ALL).is_ok());
}

#[test]
fn an_unknown_key_is_refused_by_both() {
    for text in [
        format!("{ALL}\nunknown = 1\n"),
        ALL.replace("[meet]", "[meet]\nunknown = 1"),
    ] {
        assert!(check_config(&text, Mode::Closed, None, false).is_err());
        assert!(Config::parse(&text).is_err(), "{text}");
    }
}

#[test]
fn the_meeting_place_is_the_same_three_limits_and_the_same_refusals() {
    let good = "enable_metrics = false\n[access]\nallowlist = []\n\
                [tls]\ncert_mode = \"Manual\"\n\
                [meet]\nenable = true\nrate_per_ip = 10\nmax_plates = 100\nttl_seconds = 600\n";
    assert!(check_config(good, Mode::Closed, None, false).is_ok());
    assert!(Config::parse(good).is_ok());

    let missing = "enable_metrics = false\n[access]\nallowlist = []\n\
                   [tls]\ncert_mode = \"Manual\"\n\
                   [meet]\nenable = true\nrate_per_ip = 10\n";
    assert!(check_config(missing, Mode::Closed, None, false).is_err());
    assert!(Config::parse(missing).is_err());

    let acme = "enable_metrics = false\n[access]\nallowlist = []\n\
                [tls]\ncert_mode = \"LetsEncrypt\"\n\
                [meet]\nenable = true\nrate_per_ip = 10\nmax_plates = 100\nttl_seconds = 600\n";
    assert!(check_config(acme, Mode::Closed, None, false).is_err());
    assert!(matches!(Config::parse(acme), Err(ConfigError::CertMode)));
}

#[test]
fn a_meeting_place_that_is_off_is_absent_and_needs_no_limits() {
    let off = "enable_metrics = false\n[access]\nallowlist = []\n\
               [tls]\ncert_mode = \"Manual\"\n[meet]\nenable = false\n";
    let config = Config::parse(off).unwrap();
    assert!(config.meet.is_none());
}

#[test]
fn no_tls_serves_the_relay_on_the_plain_address() {
    let text = "enable_metrics = false\nhttp_bind_addr = \"127.0.0.1:0\"\n\
                [access]\nallowlist = []\n";
    let config = Config::parse(text).unwrap();
    assert!(config.tls.is_none());
    assert_eq!(config.relay_bind_addr(), config.http_bind_addr);
    assert!(config.metrics_addr.is_none());
}
