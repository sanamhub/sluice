# 0010. `noq` performance is gated by measurement before the plan proceeds

**Status:** Accepted

## Context

Since v0.96, iroh's QUIC is `noq`, a hard fork of quinn maintained by n0, carrying QUIC Multipath, QUIC
NAT Traversal, and QUIC Address Discovery. Every quinn benchmark previously cited in this plan describes
a different codebase. `noq` has no published throughput numbers, and it sits directly beneath a project
whose headline claim is speed.

## Decision

M0 answers five questions with numbers before product code exists:

1. LAN throughput, NVMe to NVMe, against an `iperf3` ceiling.
2. Throughput on a 100 ms, 0.1% loss path.
3. Which congestion controller is in use, and whether BBR or equivalent is available.
4. Whether GSO on send and GRO on receive are enabled.
5. Whether `noq` accepts an injected rustls provider, so `aws-lc-rs` can be pinned and the negotiated
   group asserted in a test. See ADR 0012: this is no longer a question about rustls, it is a question
   about whether `noq` exposes the seam.

Within 20% of ceiling on LAN and the plan proceeds unchanged. Close and we contribute patches upstream.
Far and we add a second transport through **iroh's own custom transport extension point**, shipped in
0.97, for paths that need no NAT traversal. That is an implementation, not a fork.

## Consequences

- The riskiest assumption is tested in week one, at a cost of about two days.
- Publishing the results is a contribution in itself, since nobody has benchmarked this stack publicly.
- If the stack cannot reach the targets, they are revised in public with the measurement attached. A
  target quietly dropped is worse than a target missed.
