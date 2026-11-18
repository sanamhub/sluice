//! One frame for one connection, as the meeting place returns it.

use super::ConnectionId;
use crate::frame::ServerFrame;

/// One frame for one connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outbound {
    /// The connection to send it on.
    pub to: ConnectionId,
    /// The frame to send.
    pub frame: ServerFrame,
}
