//! The session layer: endpoints, the provider's access policy, the `sluice/1` greeting and the
//! events a transfer reports. Commands start a runtime; this module only provides futures.

mod endpoint;
mod events;
mod expiry;
mod fetch;
mod hello;
mod live;
mod net;
mod policy;
mod rate;
mod recv;
mod recv_range;
mod recv_stdout;
mod send;
mod storage;

use std::time::{SystemTime, UNIX_EPOCH};

pub use endpoint::{Congestion, N0_WARNING, RelayModeChoice};
pub use events::{Event, PathKind, emit, watch_path};
pub use expiry::{DEFAULT_SECONDS, deadline, parse as parse_expiry};
pub use hello::{
    ALPN, CAP_DELTA, CAP_LIVE, CAP_MANIFEST_V1, CAP_PACKS, CAP_RECEIPTS, CAP_ZSTD, CAPABILITIES,
    Hello, MAX_FRAME_BYTES, PROTO_VERSION, SluiceProtocol, decode as decode_hello,
    encode as encode_hello, greet,
};
pub use live::{
    CLOSE_AFTER, LiveLog, LiveSend, prepare_live, prepare_live_with_rate, receive_live,
};
pub use net::{NetworkReport, report as network_report};
pub use rate::Rate;
pub use recv::{ReceiveOptions, Received, receive, top_level_rule};
pub use recv_range::receive_range;
pub use recv_stdout::receive_stdout;
pub use send::{
    Outgoing, StopEnd, default_expiry, prepare as prepare_send,
    prepare_paths as prepare_send_paths, prepare_paths_with_rate as prepare_send_paths_with_rate,
    prepare_stdin as prepare_send_stdin, prepare_stdin_with_rate as prepare_send_stdin_with_rate,
    prepare_with_rate as prepare_send_with_rate,
};

mod provider;
mod receiver;

pub use provider::{Provider, ServeEnd, Stop};
pub use receiver::Receiver;

/// The QUIC application close code a receiver closes its content connection with once every file
/// has been verified and installed. A one-download sender that sees it stops serving at once,
/// because this is the only claim about the receiver's disk a sender can check: the byte count
/// says what left this machine, not what arrived. SPEC section 3 defines it.
pub const COMPLETE_CLOSE: u32 = 0x534c_0001;

/// The one wall clock a compare reads: UTC milliseconds since the Unix epoch. A ticket's boundary
/// and a sender's deadline are the same instant, so they are read the same way.
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

#[cfg(test)]
mod tests;
