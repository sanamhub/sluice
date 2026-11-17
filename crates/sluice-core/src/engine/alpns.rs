//! Every ALPN `SPEC.md` section 3 names, in the order a listener registers them, and whether this
//! build serves each one. iroh passes the registered names to the endpoint in registration order,
//! and a connection that offers several takes the first one the listener registered and the dialer
//! offered, so this order is the negotiation rule. The `alpn/` vectors check it.

use super::ALPN;

/// Static content, owned by `iroh-blobs`, whose release pins the number.
pub const ALPN_BLOBS: &[u8] = iroh_blobs::ALPN;

/// Pairing a device with a daemon. ADR 0039, `SPEC.md` section 11.2. Not served yet.
pub const ALPN_PAIR: &[u8] = b"/sluice/pair/1";

/// A paired device hands the daemon a ticket. ADR 0039, `SPEC.md` section 11.3. Not served yet.
pub const ALPN_DELIVER: &[u8] = b"/sluice/deliver/1";

/// Remote control of a daemon. ADR 0039, `SPEC.md` section 11.4. Not served yet.
pub const ALPN_CONTROL: &[u8] = b"/sluice/control/1";

/// Every ALPN of `SPEC.md` section 3, in registration order, and whether this build serves it.
/// Pairing and delivery are served; the control listener ships in 1.2, and until then a
/// connection that offers only its name is closed at the handshake.
pub const ALPN_REGISTERED: [(&[u8], bool); 5] = [
    (ALPN_BLOBS, true),
    (ALPN, true),
    (ALPN_PAIR, true),
    (ALPN_DELIVER, true),
    (ALPN_CONTROL, false),
];
