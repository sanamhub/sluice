//! `sluice pair` and `sluice devices`: the two lists of a pairing. `pair` makes or redeems a
//! ticket and keeps this device's list; `devices` manages the grants of a running daemon.

use clap::{Args, Subcommand, ValueEnum};
use sluice_api::proto::{
    self, CreatePairingRequest, ListDevicesRequest, RenameDeviceRequest, RevokeDeviceRequest,
};
use sluice_core::device::default_label;
use sluice_core::engine::{self, PairOutcome};
use sluice_core::paired::{Paired, PairedDaemon};
use sluice_core::ticket::{Body, Ticket};
use sluice_core::{Error, Home};

use crate::output::{Format, Output};

/// What a pairing ticket gives.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub enum GrantArg {
    /// Hand the daemon content for its inbox, and nothing else.
    #[default]
    Deliver,
    /// Every call of the control interface. Reserved for 1.2, and refused today.
    Control,
}

impl From<GrantArg> for proto::Grant {
    fn from(value: GrantArg) -> Self {
        match value {
            GrantArg::Deliver => Self::Deliver,
            GrantArg::Control => Self::Control,
        }
    }
}

/// Pairs this device with a daemon, or redeems a ticket another daemon printed, or lists the
/// daemons this device has paired with.
#[derive(Args)]
#[command(group(
    clap::ArgGroup::new("what")
        .args(["redeem", "list"])
        .multiple(false)
))]
pub struct PairArgs {
    /// The grant the ticket gives
    #[arg(long, value_enum, default_value_t = GrantArg::Deliver, value_name = "GRANT")]
    pub grant: GrantArg,
    /// How long the ticket works, in seconds; the default is 10 minutes and the most is one hour
    #[arg(long, value_name = "SECONDS")]
    pub ttl: Option<u64>,
    /// A name. For a ticket, the label the daemon writes for the device that redeems it; for a
    /// redemption, what this device calls the daemon; for a rename, the new label
    #[arg(long, value_name = "NAME")]
    pub name: Option<String>,
    /// Redeem this ticket from the daemon that printed it
    #[arg(long, value_name = "TICKET")]
    pub redeem: Option<String>,
    /// Show the daemons this device has paired with, and what they granted
    #[arg(long)]
    pub list: bool,
}

/// What a device call asks a running daemon.
#[derive(Subcommand)]
pub enum DevicesAction {
    /// The devices the daemon has granted, with their labels and pairing times
    List,
    /// Change one device's label and nothing else
    Rename {
        /// The device's label, or a prefix of its endpoint id
        device: String,
        /// The new label
        label: String,
    },
    /// Remove a device's grant and close its connections
    Revoke {
        /// The device's label, or a prefix of its endpoint id
        device: String,
    },
}

/// Manages the devices a daemon has granted.
#[derive(Args)]
pub struct DevicesArgs {
    #[command(subcommand)]
    pub action: DevicesAction,
}

/// Runs `sluice pair`.
pub fn run_pair(args: &PairArgs, output: &Output) -> Result<u8, Error> {
    let runtime = runtime()?;
    runtime.block_on(async {
        if args.list {
            return list_paired(output);
        }
        match &args.redeem {
            Some(ticket) => redeem(args, output, ticket).await,
            None => create(args, output).await,
        }
    })
}

/// Runs `sluice devices`.
pub fn run_devices(args: &DevicesArgs, output: &Output) -> Result<u8, Error> {
    let runtime = runtime()?;
    runtime.block_on(async {
        match &args.action {
            DevicesAction::List => list_devices(output).await,
            DevicesAction::Rename { device, label } => {
                let mut client = client().await?;
                let devices = client
                    .list_devices(ListDevicesRequest {})
                    .await
                    .map_err(answered)?
                    .into_inner()
                    .devices;
                let at = find_device(&devices, device)?;
                let renamed = client
                    .rename_device(RenameDeviceRequest {
                        endpoint_id: at.endpoint_id.clone(),
                        label: label.clone(),
                    })
                    .await
                    .map_err(answered)?
                    .into_inner()
                    .device;
                match output.format() {
                    Format::Human => output.write_stdout(&format!(
                        "renamed {} to {}\n",
                        at.endpoint_id,
                        renamed
                            .as_ref()
                            .map_or(label.as_str(), |device| device.label.as_str())
                    )),
                    Format::Json => output.write_stdout(&format!(
                        "{}\n",
                        serde_json::json!({
                            "renamed": at.endpoint_id,
                            "label": renamed.as_ref().map(|device| device.label.clone()),
                        })
                    )),
                }?;
                Ok(0)
            }
            DevicesAction::Revoke { device } => {
                let mut client = client().await?;
                let devices = client
                    .list_devices(ListDevicesRequest {})
                    .await
                    .map_err(answered)?
                    .into_inner()
                    .devices;
                let at = find_device(&devices, device)?;
                let revoked = client
                    .revoke_device(RevokeDeviceRequest {
                        endpoint_id: at.endpoint_id.clone(),
                    })
                    .await
                    .map_err(answered)?
                    .into_inner()
                    .revoked;
                match output.format() {
                    Format::Human => output.write_stdout(&format!(
                        "{} {}\n",
                        if revoked { "revoked" } else { "no grant for" },
                        at.endpoint_id
                    )),
                    Format::Json => output.write_stdout(&format!(
                        "{}\n",
                        serde_json::json!({ "revoked": revoked, "endpoint_id": at.endpoint_id })
                    )),
                }?;
                Ok(0)
            }
        }
    })
}

/// Asks a running daemon for a ticket and prints it. The ticket is secret material and is the one
/// line this command writes to standard output.
async fn create(args: &PairArgs, output: &Output) -> Result<u8, Error> {
    let mut client = client().await?;
    let answer = client
        .create_pairing(CreatePairingRequest {
            grant: proto::Grant::from(args.grant) as i32,
            ttl_seconds: args.ttl.unwrap_or(0),
            label: args.name.clone().unwrap_or_default(),
        })
        .await
        .map_err(answered)?
        .into_inner();
    let ticket = answer
        .ticket
        .ok_or_else(|| Error::Network("the daemon answered no ticket".to_string()))?;
    crate::events::ticket(*output, &ticket.text)?;
    Ok(0)
}

/// Redeems a ticket from this device, and keeps the daemon under a name in this home.
async fn redeem(args: &PairArgs, output: &Output, text: &str) -> Result<u8, Error> {
    let home = Home::open(&Home::locate()?)?;
    let ticket = Ticket::decode(text)?;
    if !matches!(ticket.body(), Body::Pairing { .. }) {
        return Err(Error::Policy(
            "this is not a pairing ticket; redeem a ticket `sluice pair` printed".to_string(),
        ));
    }
    match engine::redeem(&home, text).await? {
        PairOutcome::Accepted(grant) => {
            let name = args
                .name
                .clone()
                .unwrap_or_else(|| default_label(ticket.endpoint_id()));
            let daemon = PairedDaemon {
                name: name.clone(),
                endpoint_id: *ticket.endpoint_id(),
                addrs: ticket.addrs().to_vec(),
                relay: ticket.relay_url().map(str::to_string),
                grant,
                paired_ms: engine::now_ms(),
            };
            Paired::open(home.path()).add(daemon)?;
            match output.format() {
                Format::Human => output.write_stdout(&format!(
                    "paired with {name} ({}), grant {grant}\n",
                    sluice_core::paired::hex(ticket.endpoint_id())
                )),
                Format::Json => output.write_stdout(&format!(
                    "{}\n",
                    serde_json::json!({
                        "paired": true,
                        "name": name,
                        "endpoint_id": sluice_core::paired::hex(ticket.endpoint_id()),
                        "grant": grant.as_str(),
                    })
                )),
            }?;
            Ok(0)
        }
        PairOutcome::Refused => Err(Error::Policy(
            "the daemon refused this pairing ticket: it expired, was already redeemed, or the \
             grant it made was revoked"
                .to_string(),
        )),
        PairOutcome::Unreadable => Err(Error::Storage(
            "the daemon could not read or write its grants file".to_string(),
        )),
    }
}

/// The daemons this device paired with.
fn list_paired(output: &Output) -> Result<u8, Error> {
    let home = Home::open(&Home::locate()?)?;
    let daemons = Paired::open(home.path()).list()?;
    match output.format() {
        Format::Human => {
            let mut text = String::new();
            for daemon in &daemons {
                text.push_str(&format!(
                    "{}\t{}\t{}\n",
                    daemon.name,
                    daemon.grant,
                    sluice_core::paired::hex(&daemon.endpoint_id)
                ));
            }
            output.write_stdout(&text)?;
        }
        Format::Json => {
            let daemons: Vec<_> = daemons
                .iter()
                .map(|daemon| {
                    serde_json::json!({
                        "name": daemon.name,
                        "endpoint_id": sluice_core::paired::hex(&daemon.endpoint_id),
                        "grant": daemon.grant.as_str(),
                        "paired_ms": daemon.paired_ms,
                    })
                })
                .collect();
            output.write_stdout(&format!("{}\n", serde_json::json!({ "daemons": daemons })))?;
        }
    }
    Ok(0)
}

/// The devices a running daemon has granted.
async fn list_devices(output: &Output) -> Result<u8, Error> {
    let mut client = client().await?;
    let devices = client
        .list_devices(ListDevicesRequest {})
        .await
        .map_err(answered)?
        .into_inner()
        .devices;
    match output.format() {
        Format::Human => {
            let mut text = String::new();
            for device in &devices {
                text.push_str(&format!(
                    "{}\t{}\t{}\n",
                    device.label,
                    grant_name(device.grant),
                    device.endpoint_id
                ));
            }
            output.write_stdout(&text)?;
        }
        Format::Json => {
            let devices: Vec<_> = devices
                .iter()
                .map(|device| {
                    serde_json::json!({
                        "endpoint_id": device.endpoint_id,
                        "label": device.label,
                        "grant": grant_name(device.grant),
                        "paired_ms": device.paired_ms,
                    })
                })
                .collect();
            output.write_stdout(&format!("{}\n", serde_json::json!({ "devices": devices })))?;
        }
    }
    Ok(0)
}

/// A device by its label, or by a prefix of its endpoint id. A prefix that names more than one is
/// refused.
fn find_device(devices: &[proto::DeviceInfo], wanted: &str) -> Result<proto::DeviceInfo, Error> {
    if let Some(found) = devices.iter().find(|device| device.label == wanted) {
        return Ok(found.clone());
    }
    let prefix = wanted.to_ascii_lowercase();
    if prefix.is_empty() || !prefix.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(no_device(wanted));
    }
    let mut matches = devices
        .iter()
        .filter(|device| device.endpoint_id.to_ascii_lowercase().starts_with(&prefix));
    match (matches.next(), matches.next()) {
        (Some(found), None) => Ok(found.clone()),
        (Some(_), Some(_)) => Err(Error::Policy(format!(
            "{wanted} names more than one device; use the whole endpoint id"
        ))),
        _ => Err(no_device(wanted)),
    }
}

fn no_device(wanted: &str) -> Error {
    Error::Policy(format!(
        "no device is called {wanted}; `sluice devices list` shows them"
    ))
}

fn grant_name(grant: i32) -> &'static str {
    match proto::Grant::try_from(grant) {
        Ok(proto::Grant::Deliver) => "deliver",
        Ok(proto::Grant::Control) => "control",
        _ => "unknown",
    }
}

/// The daemon's local control client.
async fn client() -> Result<sluice_api::Client, Error> {
    sluice_api::connect(&Home::locate()?)
        .await
        .map_err(crate::daemon::refused)
}

/// A refusal from the daemon, as the kind of error that says what happened.
fn answered(status: tonic::Status) -> Error {
    let text = format!("the daemon answered: {}", status.message());
    match status.code() {
        tonic::Code::PermissionDenied | tonic::Code::InvalidArgument => Error::Policy(text),
        tonic::Code::Unavailable => Error::Network(text),
        _ => Error::Storage(text),
    }
}

/// The runtime `pair` and `devices` run on.
fn runtime() -> Result<tokio::runtime::Runtime, Error> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| Error::Storage(format!("cannot start the runtime: {e}")))
}
