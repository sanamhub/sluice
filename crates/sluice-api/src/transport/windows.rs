//! The named pipe: a pipe whose access list names only this user, `SYSTEM` and `Administrators`.
//! A peer is this user or it is refused, by the user of the token of the process at the other end.

use std::ffi::OsString;
use std::io;
use std::path::Path;
use std::time::Duration;

use tokio::net::windows::named_pipe::{
    ClientOptions, NamedPipeClient, NamedPipeServer, PipeMode, ServerOptions,
};

use super::{Incoming, accepting};
use crate::error::ApiError;

mod security;

/// The stream of an accepted connection.
pub(crate) type Stream = NamedPipeServer;

/// `ERROR_PIPE_BUSY`: every instance of the pipe has a client, and one is free in a moment.
const PIPE_BUSY: i32 = 231;

/// How long a client waits between tries while the pipe is busy.
const BUSY_RETRY: Duration = Duration::from_millis(50);

/// How many times a client tries a busy pipe.
const BUSY_TRIES: u32 = 100;

/// Where nothing is left to remove: a pipe goes with the last handle to it.
pub(crate) struct Guard;

/// The name of the pipe of the home at `home`. It is a hash of the home's path, so two homes have
/// two pipes, and the name says nothing about the user or the folder.
pub(crate) fn target(home: &Path) -> OsString {
    let absolute = std::path::absolute(home).unwrap_or_else(|_| home.to_path_buf());
    let text = absolute.to_string_lossy().to_lowercase();
    let text = text.trim_end_matches(['\\', '/']);
    let digest = blake3::hash(text.as_bytes()).to_hex();
    format!(r"\\.\pipe\sluice-control-{}", &digest.as_str()[..32]).into()
}

/// What to show a user of where the daemon listens.
pub(crate) fn describe(target: &OsString) -> String {
    target.to_string_lossy().into_owned()
}

/// Whether the SID of a peer is this user's. Takes both as values so a test can try another one.
pub(crate) fn check_sid(peer: &[u8], own: &[u8]) -> Result<(), ApiError> {
    if !peer.is_empty() && peer == own {
        Ok(())
    } else {
        Err(ApiError::Refused(
            "a connection from another user was refused: this daemon serves the user it runs as"
                .to_string(),
        ))
    }
}

fn instance(name: &str, sddl: &str, first: bool) -> io::Result<NamedPipeServer> {
    let mut options = ServerOptions::new();
    options
        .first_pipe_instance(first)
        .reject_remote_clients(true)
        .pipe_mode(PipeMode::Byte);
    security::create(&options, name, sddl)
}

/// Makes the pipe and starts accepting. The first instance is made with `first_pipe_instance`, so a
/// pipe of this name that another process made is an error and never one this daemon joins.
pub(crate) fn listen(home: &Path) -> Result<(Incoming<NamedPipeServer>, Guard, String), ApiError> {
    let name = describe(&target(home));
    let own = security::own_sid().map_err(|e| failed("read the user of this process", &e))?;
    let sddl = security::private_sddl(&own).map_err(|e| failed("build the access list", &e))?;
    let mut waiting = instance(&name, &sddl, true).map_err(|e| {
        ApiError::Failed(format!(
            "cannot make the pipe {name}: {e}; another daemon may be using it"
        ))
    })?;
    let shown = name.clone();
    let incoming = accepting(move || {
        // The next instance exists before this one is waited on, so there is never a moment in
        // which no instance of the pipe is there for a client to open.
        let listening =
            instance(&name, &sddl, false).map(|next| std::mem::replace(&mut waiting, next));
        let own = own.clone();
        async move {
            let server = listening?;
            server.connect().await?;
            let peer = security::client_sid(&server);
            match peer
                .map_err(|e| {
                    ApiError::Refused(format!("the other end could not be identified: {e}"))
                })
                .and_then(|peer| check_sid(&peer, &own))
            {
                Ok(()) => Ok(Some(server)),
                Err(refused) => {
                    tracing::warn!("{refused}");
                    Ok(None)
                }
            }
        }
    });
    Ok((incoming, Guard, shown))
}

/// Connects to the pipe at `target`, and checks that the process that made it runs as this user.
pub(crate) async fn open(target: &OsString) -> io::Result<NamedPipeClient> {
    let mut tries = 0;
    let client = loop {
        match ClientOptions::new().open(target) {
            Ok(client) => break client,
            Err(e) if e.raw_os_error() == Some(PIPE_BUSY) && tries < BUSY_TRIES => {
                tries += 1;
                tokio::time::sleep(BUSY_RETRY).await;
            }
            Err(e) => return Err(e),
        }
    };
    let server = security::server_sid(&client)?;
    let own = security::own_sid()?;
    check_sid(&server, &own).map_err(io::Error::other)?;
    Ok(client)
}

fn failed(what: &str, error: &io::Error) -> ApiError {
    ApiError::Failed(format!("cannot {what}: {error}"))
}

#[cfg(test)]
mod tests;
