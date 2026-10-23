//! iroh-relay 1.4.0's own example configs, from its tests and README, copied into
//! `testdata/iroh-relay-1.4.0` with the lines each came from. Every one must parse through the
//! guard's schema: a config iroh-relay documents that the guard cannot read would mean the schema
//! is missing a key.

use crate::{Mode, check_config, schema};

fn examples() -> Vec<(String, String)> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/iroh-relay-1.4.0");
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|e| e == "toml"))
        .collect();
    files.sort();
    files
        .into_iter()
        .map(|path| {
            let name = path.file_stem().unwrap().to_string_lossy().into_owned();
            (name, super::read(&path))
        })
        .collect()
}

#[test]
fn every_example_of_iroh_relay_parses_through_the_schema() {
    let examples = examples();
    assert_eq!(examples.len(), 20);
    for (name, text) in examples {
        if let Err(err) = toml::from_str::<schema::Config>(&text) {
            panic!("{name}: {}", err.message());
        }
    }
}

#[test]
fn each_example_is_closed_open_or_refused_for_its_ids() {
    for (name, text) in examples() {
        // Only the pebble example writes `enable_metrics`, which the guard requires.
        let text = match text.contains("enable_metrics") {
            true => text,
            false => super::m(&text),
        };
        // The examples are checked for their access rules, not for `[tls]`: one without it is a
        // relay behind a proxy, so it gets the flag rather than a refusal for the rule under test
        // here. ADR 0021's rule has tests of its own.
        let behind_proxy = !text.contains("[tls]");
        let closed = check_config(&text, Mode::Closed, None, behind_proxy).is_ok();
        let rated = match text.contains("[limits.client.rx]") {
            true => text.clone(),
            false => format!("{text}\n[limits.client.rx]\nbytes_per_second = 1\n"),
        };
        let open = check_config(&rated, Mode::Open, None, behind_proxy).is_ok();
        let rendezvous = check_config(&text, Mode::Rendezvous, None, false).is_ok();
        let want = match name.as_str() {
            // The README's placeholder is not an id, in either list.
            "readme-allowlist" | "readme-denylist" => (false, false, false),
            // Their URL has a query, `?boo=baz`, which iroh-relay would log (ADR 0020).
            "access-http-bearer" | "access-http-url" => (false, false, false),
            // iroh-relay refuses these two at start, as its own test_access_token_empty_is_rejected
            // shows (main.rs:920-939), and the guard now refuses them first.
            "access-shared-token-empty" | "access-shared-token-empty-string" => {
                (false, false, false)
            }
            // The relay half off, and nothing else set: a rendezvous config.
            "enable-relay-false" => (false, false, true),
            n if n.contains("http") || n.contains("token") || n == "access-allowlist" => {
                (true, false, false)
            }
            _ => (false, true, false),
        };
        assert_eq!((closed, open, rendezvous), want, "{name}");
    }
}
