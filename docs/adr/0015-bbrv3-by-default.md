# 0015. BBRv3 is the default congestion controller

**Status:** Accepted
**Date:** 2026-10-19
**Approved by:** @sanamhub
**Relates to:** ADR 0010

## Context

`noq` ships Cubic, BBRv3 and NewReno, swappable through
`TransportConfig::congestion_controller_factory`. Cubic is the default.

Cubic treats loss as congestion. On the 200 ms, 2% loss path in `docs/performance.md` it backs off on
almost every round trip, and the 300 Mbit/s target is out of reach. On iroh#4286, Cubic on a LAN moved
1.3 to 1.7 MB/s where BBR moved 42.65 MB/s. The cause there is not known, and it was on macOS with an
older `noq`, but no measurement shows Cubic doing well on this stack.

## Decision

Sluice configures BBRv3 on every connection it opens. `--congestion cubic` remains available for
comparison and for networks where BBR misbehaves.

Both are measured on every scenario, so this default is confirmed or reversed by numbers.

## Consequences

- Lossy long-distance paths get a controller designed for them.
- BBR is known to take more than its share against Cubic flows on shared bottlenecks. A transfer tool
  that starves a household's video calls is a bad neighbour, so the bandwidth caps in the transfer
  queue (`docs/design.md`) are the user's control for that.
- BBRv3 is still an IETF draft (`draft-ietf-ccwg-bbr`), and `noq`'s implementation has had less
  production use than its Cubic.
