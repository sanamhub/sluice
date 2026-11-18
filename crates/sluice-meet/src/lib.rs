//! Short codes: a person reads `<nameplate>-<word>-<word>` and two online peers meet at a relay.
//!
//! The crate is transport-free. It holds the code and its word list, the SPAKE2 exchange and the
//! sealed ticket, the frames of `GET /meet`, the client state machine that both peers drive, and the
//! relay's meeting-place state machine. Time is passed in; nothing here reads a clock, opens a
//! socket or touches a file.
//!
//! # The code is secret
//!
//! [`Code`] has no `Display` and no `Serialize`. Its `Debug` shows the nameplate, which the relay
//! sees, and never the words. `Code::text` is the one way to turn it back into the string a person
//! types, and a caller does that only to show the sender's own new code. Every error type in this
//! crate names a rule and never the code, a word, a ticket or a key.
//!
//! # `spake2` has no independent audit
//!
//! `spake2` 0.4.0, the newest stable release and the one ADR 0041 pins, has no independent audit by
//! its own README. It is the exchange that keeps a code's one guess online, but it is not audited,
//! and documentation must not say that it is.
//!
//! # The shape of a meeting
//!
//! A sender claims a nameplate, a receiver joins it, both run [`Exchange`] with the two words as the
//! password, each confirms, and the sender seals the ticket. [`client::Sender`] and
//! [`client::Receiver`] turn server frames into the few frames and events a transport needs.
//! [`meet::MeetingPlace`] does the relay's half: plates, roles, burn, limits and expiry. Neither
//! half knows what a ticket or a word means.
//!
//! ```no_run
//! use sluice_meet::Code;
//!
//! let code = Code::parse("7-orbit-velvet")?;
//! # Ok::<(), sluice_meet::CodeError>(())
//! ```

pub mod client;
pub mod code;
pub mod crypto;
pub mod frame;
pub mod meet;
pub mod words;

pub use code::{Code, CodeError, MAX_NAMEPLATE};
pub use crypto::{CryptoError, Exchange, Keys};
pub use frame::{
    ClientFrame, ErrorCode, FailReason, FrameError, MAX_FRAME, MAX_TICKET, Role, ServerFrame,
};
pub use meet::{Config, ConfigError, ConnectionId, MeetingPlace, Outbound};
