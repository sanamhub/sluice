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
2. `--congestion bbr3` stays selectable for measurement and for paths where Cubic underperforms.
3. The default returns to BBRv3 only after the release with the fix is in the tree, a loopback
   smoke run passes, and the lossy S5 path in `docs/performance.md` is measured against Cubic. The
   measurement, not the release note alone, decides.
4. Windows stay at `noq`'s defaults in this batch. The bandwidth-delay product sizing in
   ADR 0016 waits for the transport measurement, and so do several connections.

This ADR amends only the default in ADR 0015. Its other reasoning, the warning about BBR sharing a
bottleneck, and `--congestion` as the comparison switch all stand.

## Consequences

- A transfer uses the controller with the longer production record, at the cost of the lossy-path
  throughput ADR 0015 wanted.
- The BBRv3 code path remains compiled and tested, so switching back is a one-line default change
  once the condition is met.
- The switch is recorded in every result and every progress line, so a measurement says which
  controller produced it.
