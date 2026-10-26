# 0023. Cubic is the default until the BBRv3 max_bw fix ships

**Status:** Accepted **Date:** 2026-10-26 **Approved by:** @sanamhub **Amends:** ADR 0015

## Context

ADR 0015 made BBRv3 the default because loss-based Cubic had measured badly on this stack. Its
evidence was one LAN result on an older `noq`, with the cause unknown, and the decision said that
measurements would confirm or reverse it.

`noq` 1.4.0's BBRv3 has a max bandwidth bug, tracked as noq#852 and fixed by quinn-rs/quinn#2481,
which is not merged in the release we use. A controller that estimates the bottleneck from a
broken maximum can over-send or under-send for a whole transfer, and the error is not visible in
the API. Shipping it as the default would make every transfer an unmeasured experiment.

Cubic is still `noq`'s default and has had more production use.

## Decision

1. Sluice opens every connection with Cubic until a `noq` release contains the max bandwidth fix
   from quinn-rs/quinn#2481.
2. BBRv3 stays available for measurement in `bench/probe` (`--congestion bbr3`). The `sluice`
   command gets the same `--congestion` switch that ADR 0015 promised before 1.0, so a user on a
   path where Cubic underperforms can choose BBRv3.
3. The default returns to BBRv3 only after the release with the fix is in the tree, a loopback
   smoke run passes, and the lossy S5 path in `docs/performance.md` is measured against Cubic. The
   measurement, not the release note alone, decides.
4. Flow-control windows stay at `noq`'s defaults for now. The bandwidth-delay product sizing in
   ADR 0016 waits for the transport measurement, and so do several connections.

This ADR amends only the default in ADR 0015. Its other reasoning, the warning about BBR sharing a
bottleneck, and `--congestion` as the comparison switch all stand.

## Consequences

- A transfer uses the controller with the longer production record, at the cost of the lossy-path
  throughput ADR 0015 wanted.
- The BBRv3 code path remains compiled and tested in `bench/probe`, so switching back is a one-line
  default change once the condition is met.
- Every probe result records the controller, so a measurement says which one produced it. The
  `sluice` command's `--json` events should name it too once the switch exists.
