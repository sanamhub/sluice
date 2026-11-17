//! The device calls: making a pairing ticket, and listing, renaming and revoking the devices a
//! daemon has granted. A pairing ticket is secret material, so it is in the answer and nowhere else.

use sluice_core::device::{Device, Grant};

use super::Service;
use crate::error::ApiError;
use crate::proto;

impl Service {
    /// Makes a pairing ticket for one device and one grant, valid for a while.
    pub fn create_pairing(
        &self,
        request: proto::CreatePairingRequest,
    ) -> Result<proto::CreatePairingResponse, ApiError> {
        let listener = self.listener()?;
        let grant = match proto::Grant::try_from(request.grant) {
            Ok(proto::Grant::Deliver) => Grant::Deliver,
            // `control` is defined so a 1.2 daemon can grant it, and refused here: a control peer
            // is the daemon's user, and 1.1 serves no control listener.
            Ok(proto::Grant::Control) => {
                return Err(ApiError::Refused(
                    "a control grant is written in 1.2; this daemon grants deliver only"
                        .to_string(),
                ));
            }
            _ => {
                return Err(ApiError::Invalid(
                    "a pairing ticket needs a grant: deliver".to_string(),
                ));
            }
        };
        let ttl = (request.ttl_seconds > 0).then_some(request.ttl_seconds);
        let label = (!request.label.trim().is_empty()).then_some(request.label.as_str());
        let ticket = listener
            .pair(grant, ttl, label)
            .map_err(|error| ApiError::from_core(&error, ""))?;
        Ok(proto::CreatePairingResponse {
            ticket: Some(proto::Ticket { text: ticket.text }),
            expires_ms: ticket.expires_ms,
        })
    }

    /// The devices this daemon has granted, or a failure that says the grants file could not be
    /// read. Never an empty list for a file that could not be read.
    pub fn list_devices(&self) -> Result<proto::ListDevicesResponse, ApiError> {
        let listener = self.listener()?;
        let devices = listener
            .devices()
            .list()
            .map_err(|error| ApiError::from_core(&error, ""))?;
        Ok(proto::ListDevicesResponse {
            devices: devices.iter().map(device_info).collect(),
        })
    }

    /// Changes one device's label and nothing else.
    pub fn rename_device(
        &self,
        request: proto::RenameDeviceRequest,
    ) -> Result<proto::RenameDeviceResponse, ApiError> {
        let listener = self.listener()?;
        let id = endpoint_id(&request.endpoint_id)?;
        let device = listener
            .devices()
            .rename(&id, &request.label)
            .map_err(|error| ApiError::from_core(&error, ""))?;
        Ok(proto::RenameDeviceResponse {
            device: Some(device_info(&device)),
        })
    }

    /// Removes a grant, forgets the ticket that made it, and closes that device's connections.
    pub fn revoke_device(
        &self,
        request: proto::RevokeDeviceRequest,
    ) -> Result<proto::RevokeDeviceResponse, ApiError> {
        let listener = self.listener()?;
        let id = endpoint_id(&request.endpoint_id)?;
        let revoked = listener
            .revoke(&id)
            .map_err(|error| ApiError::from_core(&error, ""))?;
        Ok(proto::RevokeDeviceResponse { revoked })
    }
}

/// One device as the control interface shows it. The endpoint id is lower-case hex, as a ticket
/// spells it, and no secret is copied.
fn device_info(device: &Device) -> proto::DeviceInfo {
    proto::DeviceInfo {
        endpoint_id: device
            .endpoint_id
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
        label: device.label.clone(),
        grant: grant_of(device.grant) as i32,
        paired_ms: device.paired_ms,
    }
}

fn grant_of(grant: Grant) -> proto::Grant {
    match grant {
        Grant::Deliver => proto::Grant::Deliver,
        Grant::Control => proto::Grant::Control,
    }
}

/// The 32 bytes a 64-character hex string spells.
fn endpoint_id(text: &str) -> Result<[u8; 32], ApiError> {
    let invalid = || ApiError::Invalid("an endpoint id is 64 hex characters".to_string());
    if text.len() != 64 {
        return Err(invalid());
    }
    let mut id = [0u8; 32];
    for (at, byte) in id.iter_mut().enumerate() {
        let pair = text.get(at * 2..at * 2 + 2).ok_or_else(invalid)?;
        *byte = u8::from_str_radix(pair, 16).map_err(|_| invalid())?;
    }
    Ok(id)
}
