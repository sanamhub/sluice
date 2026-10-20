//! Runs `probe serve` and `probe fetch` against an `iroh-relay` server that runs in this process with
//! its relay half off, which is what the `rendezvous` image runs, and checks what the two ends
//! learn from it and that they connect without a relay.

use std::{
    net::{Ipv4Addr, SocketAddr},
    path::PathBuf,
};

use iroh_relay::server::{QuicConfig, Server, ServerConfig};
use rustls::pki_types::PrivatePkcs8KeyDer;
use serde_json::Value;

mod common;
use common::{log_paths, probe, read_ticket, spawn_server, within_deadline};

const SIZE: &str = "8MiB";

/// A directory removed on drop, for the certificate the probe is told to trust.
struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("sluice-probe-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        Self(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// An `iroh-relay` with address discovery only, on loopback, with a certificate made for the test.
struct Rendezvous {
    // Declared first so the server stops before the runtime that drives it goes away.
    server: Server,
    _runtime: tokio::runtime::Runtime,
    dir: TempDir,
    addr: SocketAddr,
}

impl Rendezvous {
    fn start(name: &str) -> Self {
        let dir = TempDir::new(name);
        let cert = rcgen::generate_simple_self_signed(vec!["127.0.0.1".into(), "localhost".into()])
            .expect("generate a certificate");
        std::fs::write(dir.0.join("ca.pem"), cert.cert.pem()).expect("write the certificate");
        let key = PrivatePkcs8KeyDer::from(cert.signing_key.serialize_der());
        // ring, as the upstream binary uses, so the handshake is the one a real rendezvous does.
        let tls = rustls::ServerConfig::builder_with_provider(iroh_relay::tls::default_provider())
            .with_safe_default_protocol_versions()
            .expect("protocol versions")
            .with_no_client_auth()
            .with_single_cert(vec![cert.cert.der().clone()], key.into())
            .expect("server certificate");

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let mut quic = QuicConfig::new((Ipv4Addr::LOCALHOST, 0));
        quic.server_config = Some(tls);
        let mut config = ServerConfig::default();
        config.relay = None;
        config.quic = Some(quic);
        config.metrics_addr = None;
        let server = runtime
            .block_on(Server::spawn(config))
            .expect("start the server");
        let addr = server.quic_addr().expect("a QUIC address");
        assert!(
            server.http_addr().is_none() && server.https_addr().is_none(),
            "the relay half must not run"
        );
        Self {
            server,
            _runtime: runtime,
            dir,
            addr,
        }
    }

    /// The options that point a probe at it.
    fn args(&self) -> Vec<String> {
        vec![
            "--rendezvous".to_string(),
            format!("https://127.0.0.1:{}", self.addr.port()),
            "--rendezvous-ca".to_string(),
            self.dir.0.join("ca.pem").display().to_string(),
        ]
    }

    /// QUIC connections the server accepted for address discovery.
    fn discoveries(&self) -> u64 {
        self.server.metrics().server.qad_connections.get()
    }
}

/// The JSON inside a ticket line.
fn ticket_json(ticket: &str) -> Value {
    let hex = ticket.strip_prefix("sluiceprobe2").expect("ticket prefix");
    let bytes: Vec<u8> = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("hex"))
        .collect();
    serde_json::from_slice(&bytes).expect("ticket JSON")
}

fn learned(discovery: &Value) -> Vec<String> {
    discovery["learned"]
        .as_array()
        .expect("learned is an array")
        .iter()
        .map(|a| a.as_str().expect("address").to_string())
        .collect()
}

fn fetch_args(ticket: &str, extra: &[String]) -> Vec<String> {
    let mut args: Vec<String> = ["fetch", ticket, "--mode", "raw"]
        .map(str::to_string)
        .to_vec();
    args.extend_from_slice(extra);
    args
}

#[test]
fn both_ends_learn_an_address_and_connect_directly_without_a_relay() {
    within_deadline(|| {
        let rendezvous = Rendezvous::start("rv-direct");
        let (_serve, mut stdout) = spawn_server("raw", "bbr3", SIZE, &rendezvous.args());
        let ticket = read_ticket(&mut stdout);

        let carried = ticket_json(&ticket);
        let served = learned(&carried["discovery"]);
        assert!(!served.is_empty(), "serve learnt no address: {carried}");
        assert_eq!(carried["discovery"]["complete"], true);
        // On loopback iroh merges the reported address well inside the deadline, so nothing is added
        // by hand. A non-empty list here means the wait did not work.
        assert_eq!(
            carried["discovery"]["added_from_report"],
            serde_json::json!([]),
            "{carried}"
        );
        assert!(
            carried["discovery"]["took_s"]
                .as_f64()
                .is_some_and(|s| s >= 0.0)
        );
        // The ticket has to carry what the server said, or the other end cannot dial it.
        for address in &served {
            assert!(
                carried["addrs"].to_string().contains(address.as_str()),
                "{address} is not in the ticket addresses {}",
                carried["addrs"]
            );
        }

        let output = probe()
            .args(fetch_args(&ticket, &rendezvous.args()))
            .output()
            .expect("run probe fetch");
        assert!(
            output.status.success(),
            "fetch failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let result: Value =
            serde_json::from_slice(output.stdout.trim_ascii()).expect("one JSON line");
        assert_eq!(result["outcome"], "ok");
        assert_eq!(result["path"], "direct");
        assert_eq!(result["relay"], "disabled");
        assert_eq!(
            result["rendezvous"],
            format!("https://127.0.0.1:{}/", rendezvous.addr.port())
        );
        assert_eq!(result["discovery"]["complete"], true);
        assert!(!learned(&result["discovery"]).is_empty(), "{result}");
        assert!(result["connect_s"].as_f64().is_some_and(|s| s > 0.0));
        assert!(result["error"].is_null());
        // A direct connection names the address the other end was reached at, never a relay URL.
        log_paths("fetch", &result["connection_stats"][0]);
        let paths = &result["connection_stats"][0]["paths"];
        assert!(
            paths
                .as_array()
                .expect("paths")
                .iter()
                .all(|p| p["relay"] == false && p["remote_addr"].as_str().is_some()),
            "{paths}"
        );
        // One discovery connection each, so the server saw both ends and the printed addresses came
        // from it and not from the local interfaces.
        assert!(
            rendezvous.discoveries() >= 2,
            "the server answered {} connections",
            rendezvous.discoveries()
        );
    });
}

#[test]
fn a_dial_that_goes_nowhere_is_a_no_path_result() {
    within_deadline(|| {
        let rendezvous = Rendezvous::start("rv-nopath");
        let ticket = {
            let (_serve, mut stdout) = spawn_server("raw", "bbr3", SIZE, &rendezvous.args());
            read_ticket(&mut stdout)
            // `serve` is killed here, so the address in the ticket leads nowhere.
        };
        let mut args = fetch_args(&ticket, &rendezvous.args());
        args.extend(["--timeout".to_string(), "3".to_string()]);
        let output = probe().args(args).output().expect("run probe fetch");
        assert!(!output.status.success(), "a failed dial must not exit 0");
        let result: Value =
            serde_json::from_slice(output.stdout.trim_ascii()).expect("one JSON line");
        assert_eq!(result["outcome"], "no_path");
        assert!(result["connect_s"].is_null());
        assert!(result["error"].as_str().is_some_and(|e| !e.is_empty()));
        assert_eq!(result["received_bytes"], 0);
        assert!(result["key_exchange_group"].is_null());
    });
}

#[test]
fn a_rendezvous_that_does_not_answer_is_waited_for_and_then_recorded_as_empty() {
    within_deadline(|| {
        // Nothing listens on port 1, so no address can come back.
        let args = ["--rendezvous", "https://127.0.0.1:1"].map(str::to_string);
        let (_serve, mut stdout) = spawn_server("raw", "bbr3", SIZE, &args);
        let ticket = read_ticket(&mut stdout);
        let discovery = &ticket_json(&ticket)["discovery"];
        assert_eq!(discovery["complete"], false);
        assert!(learned(discovery).is_empty());
        assert!(
            discovery["took_s"]
                .as_f64()
                .is_some_and(|s| (9.9..20.0).contains(&s)),
            "{discovery}"
        );
    });
}
