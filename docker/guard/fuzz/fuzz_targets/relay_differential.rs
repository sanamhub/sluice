#![no_main]

//! The same text through the guard and through a loose mirror of how iroh-relay 1.4.0 reads it:
//! no unknown key refused, a missing `access` read as everyone, and `enable_relay` true unless
//! written (main.rs:94-159 and 370-389). Whatever the guard lets start is what iroh-relay would
//! run: closed means iroh-relay's own `access` is an allowlist, an HTTP check or shared tokens,
//! rendezvous means its relay half is off, and closed or open means it is on.

use libfuzzer_sys::fuzz_target;
use serde::Deserialize;
use sluice_relay_guard::{Mode, check_config};

/// iroh-relay 1.4.0's `Config`, as loose as it is there. Only the fields the assertions read are
/// named, and every other key is ignored, as iroh-relay ignores keys it does not know.
#[derive(Deserialize)]
struct Loose {
    #[serde(default = "on")]
    enable_relay: bool,
    #[serde(default)]
    access: LooseAccess,
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
    Http(toml::Value),
    #[serde(rename = "shared_token")]
    SharedToken(Vec<String>),
}

// The first byte picks whether IROH_RELAY_ACCESS_TOKEN is set; the rest is the config's text.
fuzz_target!(|data: &[u8]| {
    let Some((&choice, rest)) = data.split_first() else {
        return;
    };
    let text = String::from_utf8_lossy(rest);
    let token = (choice & 1 == 1).then_some("t");
    for mode in [Mode::Closed, Mode::Open, Mode::Rendezvous] {
        if check_config(&text, mode, token).is_err() {
            continue;
        }
        // The guard's schema is stricter at every point, so text it passes, the mirror reads.
        let loose: Loose = match toml::from_str(&text) {
            Ok(loose) => loose,
            Err(err) => panic!("the guard passed {mode:?} text iroh-relay cannot read: {err}"),
        };
        match mode {
            Mode::Closed => assert!(
                matches!(
                    loose.access,
                    LooseAccess::Allowlist(_) | LooseAccess::Http(_) | LooseAccess::SharedToken(_)
                ),
                "the guard passed as closed what iroh-relay runs open"
            ),
            Mode::Rendezvous => assert!(
                !loose.enable_relay,
                "the guard passed as rendezvous what iroh-relay runs as a relay"
            ),
            Mode::Open => {}
        }
        if mode != Mode::Rendezvous {
            assert!(loose.enable_relay, "the guard passed {mode:?} with the relay half off");
        }
    }
});
