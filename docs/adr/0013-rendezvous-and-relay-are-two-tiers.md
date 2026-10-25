# 0013. Rendezvous and relay ship as two separate images

**Status:** Accepted
**Date:** 2026-10-19
**Approved by:** @sanamhub
**Relates to:** ADR 0011

## Context

Two peers behind NATs need something reachable to learn their mapped addresses (rendezvous). When
hole punching fails, they need something to carry the traffic (relay). The first costs almost nothing
to run. The second costs whatever the transfers weigh.

`iroh-relay`'s `ServerConfig` has two independent optional halves: `quic`, the address discovery
server, and `relay`, the WebSocket service that carries relayed traffic and the coordination side
channel. With `relay: None` the server tells peers their own addresses and moves no transfer bytes.

What is not known is how much hole punching depends on that side channel.

## Decision

Two container images:

- `rendezvous`: address discovery only. Fits the smallest VPS. The default recommendation in the docs.
- `relay`: both halves, authorization on (ADR 0011). The image with a bandwidth bill, and the docs say
  so next to its name.

Rejected: one image with a flag. The two differ in operating cost by an order of magnitude, and a flag
hides that.

The direct-connection rate with `rendezvous` alone is measured and published next to the
connectivity table in `docs/design.md`.

## Consequences

- `sluice doctor` reports which tier it reached, because "rendezvous reachable, no relay" and "no
  infrastructure" call for different advice.
- `RelayConfig::limits.accept_conn_limit` and `accept_conn_burst` are documented upstream as not
  implemented. Only the per-client byte limit works, so that is the only limit we document until we
  build connection limiting in front of the relay.
- Two images to build, sign and attest. The release pipeline handles it as one more matrix row.
