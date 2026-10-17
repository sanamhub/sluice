# 0013. Rendezvous and relay are two deployment tiers, not one server with a flag

**Status:** Proposed

Supersedes nothing. Refines ADR 0011, which established that relay mode is never chosen silently.

## Context

Section 6 separates the *rendezvous* role, signalling only, from the *relay* role, full transfer
bandwidth, and open question 3 asked whether `iroh-relay` can actually run as the first without paying
for the second. It can.

`iroh_relay::server::ServerConfig` carries two independent optional halves:

```rust
pub struct ServerConfig {
    pub relay: Option<RelayConfig>,   // disabled if None
    pub quic: Option<QuicConfig>,     // disabled if None
    pub metrics_addr: Option<SocketAddr>,
}
```

`quic` is the QUIC Address Discovery server, conventionally on `DEFAULT_RELAY_QUIC_PORT` = 7842.
`relay` is the WebSocket service that carries both the disco side channel and relayed transfer bytes.
Setting `relay: None` yields a service that tells a peer its own mapped address and moves no transfer
bytes whatsoever.

What this does *not* settle is how much of iroh's hole punching depends on the WebSocket side channel
that `relay: None` removes. A peer can observe its own address but has no path to send a coordination
message to the other side. The cases that clearly still work are addresses carried in the ticket and
port-mapped WAN. The browser client does not work at all, which ADR 0011 already established.

## Decision

Ship **two container profiles**, not one container with a flag:

- **`rendezvous`**: `quic: Some(..)`, `relay: None`. No transit bandwidth. Fits the smallest VPS
  available. This is the default recommendation in the docs and the one `sluice doctor` names first.
- **`relay`**: both halves. Authorization on by default per ADR 0011. This is the profile with a
  bandwidth bill, and the docs say so next to the image name.

Two images rather than one flag, because the operator's cost model differs by an order of magnitude
between them and a flag hides that. An operator choosing an image has already made the decision the
bill will reflect.

M0 measures the direct-connect rate achieved with `rendezvous` alone, on the same NAT matrix as
section 6, and that number is published next to the matrix. If it is materially worse than the ~90%
that section 6 claims for "WAN, own rendezvous", section 6 is corrected rather than the measurement
being dropped.

## Consequences

- The five dollar VPS in section 6 is real and verified against the config surface, not assumed.
- `sluice doctor` reports which tier it is pointed at, because "rendezvous reachable, relay absent" and
  "no infrastructure at all" are different failures and produce different advice.
- A related correction lands with this ADR: `RelayConfig::limits.accept_conn_limit` and
  `accept_conn_burst` are documented upstream as **not implemented, no effect**. Only
  `client_rx: ClientRateLimit { bytes_per_second, max_burst_bytes }` does anything. Sections 6 and 13
  promised a connection-accept rate limit for `open` mode that does not exist. Either we implement
  accept limiting in front of the relay ourselves, or we stop writing it down. A rate limit that
  silently does nothing is worse than none, because it has been published.
- Two images is two things to build, sign, and attest. `dist` and the existing release pipeline absorb
  that; the cost is one more matrix row.
