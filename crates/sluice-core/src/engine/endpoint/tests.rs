use super::RelayModeChoice;
use crate::Error;

#[test]
fn a_relay_and_n0_together_are_refused() {
    let err = RelayModeChoice::from_flags(Some("https://relay.example/"), true).unwrap_err();
    assert!(matches!(err, Error::Policy(_)));
}

#[test]
fn no_flag_means_no_relay() {
    assert_eq!(
        RelayModeChoice::from_flags(None, false).unwrap(),
        RelayModeChoice::Disabled
    );
}

/// Every socket of an endpoint made for the loopback choice is on loopback, and none is on the
/// unspecified address a firewall would ask about.
#[tokio::test]
async fn loopback_only_binds_nothing_but_loopback() {
    let endpoint = super::bind(
        None,
        &RelayModeChoice::Loopback,
        super::Congestion::Cubic,
        None,
    )
    .await
    .unwrap();
    let sockets = endpoint.bound_sockets();
    endpoint.close().await;
    assert!(!sockets.is_empty());
    assert!(
        sockets.iter().all(|socket| socket.ip().is_loopback()),
        "{sockets:?}"
    );
}

#[test]
fn a_bad_url_names_the_form_not_the_value() {
    let err = RelayModeChoice::from_flags(Some("relay"), false)
        .unwrap()
        .iroh()
        .unwrap_err();
    assert!(err.to_string().contains("absolute https or http URL"));
}
