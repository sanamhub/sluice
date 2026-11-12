//! The same checks `sluice doctor` makes on the home, without the network checks.

use sluice_core::ErrorKind;
use sluice_core::doctor::{self, Check, Name, Status};

use super::Service;
use crate::error::ApiError;
use crate::proto::{self, CheckState};

impl Service {
    /// The local checks of the home, with the exit code `sluice doctor` would give.
    pub async fn doctor(&self) -> Result<proto::DoctorResponse, ApiError> {
        let home = self.inner.home.path().to_path_buf();
        let checks = tokio::task::spawn_blocking(move || doctor::examine_at(&home, true))
            .await
            .map_err(|_| ApiError::Failed("the checks did not finish".to_string()))?;
        let exit_code = checks
            .iter()
            .filter_map(Check::failure)
            .map(exit_code_of)
            .max()
            .unwrap_or(0);
        Ok(proto::DoctorResponse {
            checks: checks.iter().map(line).collect(),
            exit_code,
        })
    }
}

/// The exit code `sluice doctor` gives for a failed check of this kind.
fn exit_code_of(kind: ErrorKind) -> u32 {
    match kind {
        ErrorKind::Network => 3,
        ErrorKind::Verify => 4,
        ErrorKind::Storage => 5,
        ErrorKind::Policy => 6,
        ErrorKind::Cancelled => 130,
        ErrorKind::Bug => 70,
    }
}

fn line(check: &Check) -> proto::Check {
    let (state, message) = match check.status() {
        Status::Ok => (CheckState::Ok, "nothing to do"),
        Status::Warning => (
            CheckState::Warn,
            "worth a look; `sluice doctor` in a shell says what and how to fix it",
        ),
        Status::Failed => (
            CheckState::Fail,
            "this fails; `sluice doctor` in a shell says what and how to fix it",
        ),
        Status::Skipped => (CheckState::Ok, "skipped"),
    };
    proto::Check {
        name: name(check.name()).to_string(),
        state: state as i32,
        message: message.to_string(),
    }
}

fn name(name: Name) -> &'static str {
    match name {
        Name::Home => "home",
        Name::Access => "access",
        Name::Layout => "layout",
        Name::Lock => "lock",
        Name::Key => "key",
        Name::Space => "space",
    }
}
