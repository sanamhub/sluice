#![no_main]

//! The same text through the guard and through a loose mirror of how iroh-relay 1.4.1 reads it:
//! no unknown key refused, a missing `access` read as everyone, `enable_relay` true unless written,
//! and `enable_metrics` true unless written (main.rs:94-159 and 370-389). Whatever the guard lets
//! start is what iroh-relay would run: closed means iroh-relay's own `access` is an allowlist, an
//! HTTP check or shared tokens, rendezvous means its relay half is off, and closed or open means it
//! is on. `--behind-proxy` and a `[tls]` table agree. The environment variable is modelled too: a
//! value that is set but empty is refused, and a set one replaces the `shared_token` list
//! (main.rs:236-244).
//!
//! The other asserts restate the guard's own rules, such as `enable_metrics` being written or an
//! open config having a byte limit. They are regression checks: they catch a rule deleted from the
//! guard, not a difference from iroh-relay.

use libfuzzer_sys::fuzz_target;
use serde::Deserialize;
use sluice_relay_guard::{Mode, check_config};
use url::Url;

/// iroh-relay 1.4.1's `Config`, as loose as it is there. Only the fields the assertions read are
/// named, and every other key is ignored, as iroh-relay ignores keys it does not know.
#[derive(Deserialize)]
struct Loose {
    #[serde(default = "on")]
    enable_relay: bool,
    /// `Option` here, where iroh-relay defaults it to true (main.rs:145 and 384-386): the
    /// assertion is that someone wrote it, since the default serves metrics with no login.
    enable_metrics: Option<bool>,
    #[serde(default)]
    access: LooseAccess,
    limits: Option<LooseLimits>,
    tls: Option<LooseTls>,
}

/// `struct Limits`, main.rs:510-518, down to the one number open mode needs.
#[derive(Deserialize)]
struct LooseLimits {
    client: Option<LooseClient>,
}

#[derive(Deserialize)]
struct LooseClient {
    rx: Option<LooseRate>,
}

#[derive(Deserialize)]
struct LooseRate {
    bytes_per_second: Option<u32>,
}

/// `struct TlsConfig`, main.rs:399-462, down to the setting that turns HTTPS off.
#[derive(Deserialize)]
struct LooseTls {
    #[serde(default)]
    dangerous_http_only: bool,
}

/// `struct HttpAccessConfig`, main.rs:203-214. The URL is a string here, and parsed by the
/// assertion with the `url` crate, which iroh-relay reads it with.
#[derive(Deserialize)]
struct LooseHttp {
    url: String,
}

/// `cfg_defaults::enable_relay`, main.rs:376-378.
fn on() -> bool {
    true
}

/// `enum AccessConfig`, main.rs:161-201, with `Everyone` the default as there. The lists are
/// read as strings, which is looser than iroh-relay's `EndpointId`, so that the mirror reads
/// at least what iroh-relay reads.
#[derive(Deserialize, Default)]
#[serde(rename_all = "lowercase")]
enum LooseAccess {
    #[default]
    Everyone,
    Allowlist(Vec<String>),
    Denylist(Vec<String>),
    Http(LooseHttp),
    #[serde(rename = "shared_token")]
    SharedToken(Vec<String>),
}

// The first byte picks whether IROH_RELAY_ACCESS_TOKEN is set; the rest is the config's text.
fuzz_target!(|data: &[u8]| {
    let Some((&choice, rest)) = data.split_first() else {
        return;
    };
    let text = String::from_utf8_lossy(rest);
    // 0: unset, 1: set to a token, 2: set but empty. A set value replaces the list
    // (main.rs:236-240), and an empty one leaves a list of one empty token, which iroh-relay
    // refuses to start on (main.rs:241, `bail_any!`). The guard refuses it first, with exit 64
    // and a clear line.
    let token = match choice % 3 {
        0 => None,
        1 => Some("t"),
        _ => Some(""),
    };
    for mode in [Mode::Closed, Mode::Open, Mode::Rendezvous] {
        for behind_proxy in [false, true] {
            let checked = check_config(&text, mode, token, behind_proxy);
            if token == Some("") {
                assert!(
                    checked.is_err(),
                    "the guard passed a set-but-empty IROH_RELAY_ACCESS_TOKEN in {mode:?}"
                );
                continue;
            }
            if checked.is_err() {
                continue;
            }
            // The guard's schema is stricter at every point, so text it passes, the mirror reads.
            let loose: Loose = match toml::from_str(&text) {
                Ok(loose) => loose,
                Err(err) => panic!("the guard passed {mode:?} text iroh-relay cannot read: {err}"),
            };
            let rendezvous = mode == Mode::Rendezvous;
            match mode {
                Mode::Closed => assert!(
                    matches!(
                        loose.access,
                        LooseAccess::Allowlist(_)
                            | LooseAccess::Http(_)
                            | LooseAccess::SharedToken(_)
                    ),
                    "the guard passed as closed what iroh-relay runs open"
                ),
                Mode::Rendezvous => assert!(
                    !loose.enable_relay,
                    "the guard passed as rendezvous what iroh-relay runs as a relay"
                ),
                Mode::Open => assert!(
                    has_rate(&loose),
                    "the guard passed open with no per-client byte limit"
                ),
            }
            if !rendezvous {
                assert!(
                    loose.enable_relay,
                    "the guard passed {mode:?} with the relay half off"
                );
                // A relay config's [tls] and the flag are the one pair that must agree: no [tls]
                // means the operator says a proxy ends TLS, and [tls] means iroh-relay does.
                assert_eq!(
                    behind_proxy,
                    loose.tls.is_none(),
                    "the guard passed {mode:?} with --behind-proxy={behind_proxy} and [tls]={}",
                    loose.tls.is_some()
                );
            } else {
                assert!(!behind_proxy, "the guard passed rendezvous with --behind-proxy");
            }
            // From here the asserts restate the guard's own rules: they are regression checks that
            // a rule was not deleted, not a comparison with what iroh-relay does.
            assert!(
                loose.enable_metrics.is_some(),
                "the guard passed {mode:?} with enable_metrics unwritten, \
                 so iroh-relay serves metrics"
            );
            assert!(
                !loose.tls.is_some_and(|tls| tls.dangerous_http_only),
                "the guard passed {mode:?} with HTTPS turned off"
            );
            if let LooseAccess::SharedToken(tokens) = &loose.access {
                assert!(
                    token.is_some() || !tokens.is_empty(),
                    "the guard passed an empty shared_token list without IROH_RELAY_ACCESS_TOKEN"
                );
            }
            if let LooseAccess::Http(http) = &loose.access {
                plain_url(&http.url);
            }
        }
    }
});

/// Whether `limits.client.rx.bytes_per_second` is written.
fn has_rate(loose: &Loose) -> bool {
    loose
        .limits
        .as_ref()
        .and_then(|limits| limits.client.as_ref())
        .and_then(|client| client.rx.as_ref())
        .is_some_and(|rx| rx.bytes_per_second.is_some())
}

/// iroh-relay logs a failed check's URL whole, so it may hold no secret: no user name, password,
/// query or fragment, and it must be one iroh-relay can parse.
fn plain_url(text: &str) {
    let url = Url::parse(text).unwrap_or_else(|err| panic!("the guard passed a URL: {err}"));
    assert!(matches!(url.scheme(), "http" | "https"), "the guard passed another scheme");
    assert!(url.username().is_empty() && url.password().is_none(), "the guard passed userinfo");
    assert!(url.query().is_none(), "the guard passed a query");
    assert!(url.fragment().is_none(), "the guard passed a fragment");
}
