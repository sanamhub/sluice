//! The guard's `[meet]` table, through the shipped data of the meeting place itself.

use super::{m, read};
use crate::{Mode, check_config};

fn example(name: &str) -> String {
    read(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("testdata")
            .join("meet")
            .join(name),
    )
}

#[test]
fn the_meeting_place_example_starts_closed() {
    let text = example("enabled.toml");
    assert_eq!(check_config(&text, Mode::Closed, None, false), Ok(()));
}

#[test]
fn the_acme_example_is_refused_and_names_the_limit() {
    let text = example("acme.toml");
    let message = check_config(&text, Mode::Closed, None, false)
        .unwrap_err()
        .to_string();
    assert!(
        message.contains("ACME is not supported with the meeting place yet"),
        "{message}"
    );
    assert!(message.contains("cert_mode = \"Manual\""), "{message}");
}

#[test]
fn a_meeting_place_without_enable_is_not_a_meeting_place() {
    let text = m("[access]\nallowlist = []\n[meet]\nrate_per_ip = 10\n");
    assert!(check_config(&text, Mode::Closed, None, false).is_err());
}
