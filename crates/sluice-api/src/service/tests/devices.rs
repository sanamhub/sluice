//! The device calls over a service with an identity endpoint, and over one without.

use super::*;
use sluice_core::engine::Listener;

/// A service over a home with an identity endpoint, so the device calls are served.
async fn with_listener(dir: &tempfile::TempDir) -> (Service, std::sync::Arc<Listener>) {
    let home = Home::open(&dir.path().join("home")).unwrap();
    let listener = Listener::bind(&home, RelayModeChoice::Loopback, None)
        .await
        .unwrap();
    (
        Service::new(
            home,
            Roots::open(&[]).unwrap(),
            RelayModeChoice::Loopback,
            Some(listener.clone()),
        ),
        listener,
    )
}

/// A pairing ticket is made for a grant and a label, redeemed by a device, and then listed,
/// renamed and revoked through the control interface.
#[tokio::test]
async fn a_pairing_ticket_is_redeemable_listed_renamed_and_revoked() {
    let dir = tempfile::tempdir().unwrap();
    let (service, listener) = with_listener(&dir).await;
    let made = service
        .create_pairing(proto::CreatePairingRequest {
            grant: proto::Grant::Deliver as i32,
            ttl_seconds: 0,
            label: "laptop".to_string(),
        })
        .unwrap();
    let text = made.ticket.unwrap().text;
    assert!(made.expires_ms > 0);
    assert_eq!(
        sluice_core::ticket::Ticket::decode(&text)
            .unwrap()
            .endpoint_id(),
        &listener.id(),
        "the ticket names the daemon's endpoint"
    );
    let device = Home::open(&dir.path().join("device")).unwrap();
    assert_eq!(
        sluice_core::engine::redeem(&device, &text).await.unwrap(),
        sluice_core::engine::PairOutcome::Accepted(sluice_core::device::Grant::Deliver)
    );
    let devices = service.list_devices().unwrap().devices;
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].label, "laptop");
    assert_eq!(devices[0].grant, proto::Grant::Deliver as i32);
    let id = devices[0].endpoint_id.clone();

    let renamed = service
        .rename_device(proto::RenameDeviceRequest {
            endpoint_id: id.clone(),
            label: "work".to_string(),
        })
        .unwrap();
    assert_eq!(renamed.device.unwrap().label, "work");
    assert!(service.list_devices().unwrap().devices[0].endpoint_id == id);

    let revoked = service
        .revoke_device(proto::RevokeDeviceRequest {
            endpoint_id: id.clone(),
        })
        .unwrap();
    assert!(revoked.revoked);
    assert!(service.list_devices().unwrap().devices.is_empty());
    listener.stop();
}

/// Only `deliver` is written in 1.1: a control grant is refused with a sentence that says when.
#[tokio::test]
async fn a_control_grant_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (service, listener) = with_listener(&dir).await;
    let refused = service
        .create_pairing(proto::CreatePairingRequest {
            grant: proto::Grant::Control as i32,
            ttl_seconds: 0,
            label: String::new(),
        })
        .unwrap_err();
    assert!(matches!(refused, ApiError::Refused(_)));
    assert!(refused.to_string().contains("1.2"), "{refused}");
    listener.stop();
}

/// A daemon whose home has no identity key serves no device call, and says why.
#[test]
fn a_daemon_without_an_identity_serves_no_device_call() {
    let (_dir, service) = service(&[]);
    let refused = service.list_devices().unwrap_err();
    assert!(matches!(refused, ApiError::Refused(_)));
    assert!(refused.to_string().contains("identity.key"), "{refused}");
}
