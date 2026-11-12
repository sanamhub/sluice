//! The signals that stop the daemon. Every handler is registered before the daemon says where it
//! listens, so a signal that arrives once a script has read that line is never lost to a default
//! action that would kill the process with jobs running.

use sluice_core::Error;

#[cfg(unix)]
pub(super) struct Signals {
    interrupt: tokio::signal::unix::Signal,
    terminate: tokio::signal::unix::Signal,
    hangup: tokio::signal::unix::Signal,
}

#[cfg(unix)]
impl Signals {
    /// Registers SIGINT, SIGTERM and SIGHUP.
    pub(super) fn register() -> Result<Self, Error> {
        use tokio::signal::unix::{SignalKind, signal};
        let one = |kind| {
            signal(kind)
                .map_err(|e| Error::Storage(format!("cannot listen for a stop signal: {e}")))
        };
        Ok(Self {
            interrupt: one(SignalKind::interrupt())?,
            terminate: one(SignalKind::terminate())?,
            hangup: one(SignalKind::hangup())?,
        })
    }

    /// Resolves at the first of them.
    pub(super) async fn arrived(mut self) {
        tokio::select! {
            _ = self.interrupt.recv() => {}
            _ = self.terminate.recv() => {}
            _ = self.hangup.recv() => {}
        }
    }
}

#[cfg(windows)]
pub(super) struct Signals {
    c: tokio::signal::windows::CtrlC,
    brk: tokio::signal::windows::CtrlBreak,
    close: tokio::signal::windows::CtrlClose,
    logoff: tokio::signal::windows::CtrlLogoff,
    shutdown: tokio::signal::windows::CtrlShutdown,
}

#[cfg(windows)]
impl Signals {
    /// Registers Ctrl+C, Ctrl+Break and the console's close, logoff and shutdown events.
    pub(super) fn register() -> Result<Self, Error> {
        use tokio::signal::windows;
        let failed =
            |e: std::io::Error| Error::Storage(format!("cannot listen for a stop signal: {e}"));
        Ok(Self {
            c: windows::ctrl_c().map_err(failed)?,
            brk: windows::ctrl_break().map_err(failed)?,
            close: windows::ctrl_close().map_err(failed)?,
            logoff: windows::ctrl_logoff().map_err(failed)?,
            shutdown: windows::ctrl_shutdown().map_err(failed)?,
        })
    }

    /// Resolves at the first of them.
    pub(super) async fn arrived(mut self) {
        tokio::select! {
            _ = self.c.recv() => {}
            _ = self.brk.recv() => {}
            _ = self.close.recv() => {}
            _ = self.logoff.recv() => {}
            _ = self.shutdown.recv() => {}
        }
    }
}
