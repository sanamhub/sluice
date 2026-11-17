// The group's self-test: the client and server halves complete each other, and a share of the
// wrong length is refused. It does not prove interop (the native peer's test does that), only that
// the layout and the combiner agree with each other.

use crate::pq::{Hybrid, Mode, Probe, SupportedKxGroup, X25519_SHARE, provider};
use rustls::NamedGroup;

/// The client and the server halves complete each other, with the shares and the secret the
/// sizes rustls gives for this group. A self-test does not prove interop, but it does prove the
/// layout and the combiner agree with each other before a browser is involved.
#[test]
fn the_two_halves_complete_each_other() {
    let probe = Probe::new();
    let group = Hybrid {
        classical: rustls::crypto::ring::kx_group::X25519,
        probe,
    };
    let client = group.start().unwrap();
    assert_eq!(client.pub_key().len(), 1184 + X25519_SHARE);
    let server = group.start_and_complete(client.pub_key()).unwrap();
    assert_eq!(server.group, NamedGroup::X25519MLKEM768);
    assert_eq!(server.pub_key.len(), 1088 + X25519_SHARE);
    let secret = client.complete(&server.pub_key).unwrap();
    assert_eq!(secret.secret_bytes().len(), 64);
    assert_eq!(secret.secret_bytes(), server.secret.secret_bytes());
}

/// A share of the wrong length is a peer that misbehaved, not a panic.
#[test]
fn a_short_share_is_refused() {
    let group = Hybrid {
        classical: rustls::crypto::ring::kx_group::X25519,
        probe: Probe::new(),
    };
    let client = group.start().unwrap();
    assert!(client.complete(&[0u8; 8]).is_err());
    assert!(group.start_and_complete(&[0u8; 8]).is_err());
}

/// Both modes put the group they name in the offer, and the hybrid first.
#[test]
fn the_provider_offers_the_mode_it_is_given() {
    let probe = Probe::new();
    let hybrid = provider(Mode::Hybrid, probe);
    assert_eq!(hybrid.kx_groups[0].name(), NamedGroup::X25519MLKEM768);
    assert_eq!(hybrid.kx_groups[1].name(), NamedGroup::X25519);
    let classical = provider(Mode::Classical, Probe::new());
    assert_eq!(classical.kx_groups.len(), 1);
    assert_eq!(classical.kx_groups[0].name(), NamedGroup::X25519);
}
