# 0024. The congestion switch runs in `send` and `recv`

**Status:** Accepted **Date:** 2026-10-26 **Approved by:** @sanamhub **Amends:** ADR 0023

## Context

ADR 0023 held the default at Cubic and said the `sluice` command would get the `--congestion`
switch that ADR 0015 promised. That promise is now in the code: both commands take
`--congestion cubic|bbr3`, the default is still Cubic, and the `started` event names the
controller that ran, in text and in `--json`.

The record of a promise that has been kept should not read as future work, and ADR 0023 is
accepted, so it is not edited.

## Decision

1. `sluice send` and `sluice recv` accept `--congestion cubic|bbr3`. Cubic remains the default for
   the reason ADR 0023 gives.
2. The `started` event carries `congestion`, and the text progress line names it, so a transcript
   says which controller a transfer used.
3. BBRv3 is marked experimental in the help text. Choosing it is a measurement, not a promise of
   better throughput.
4. The condition for returning the default to BBRv3 in ADR 0023 is unchanged.

## Consequences

- A user on a path where Cubic underperforms can compare the two without rebuilding.
- A recorded transfer says which controller produced its numbers, which is what the S5
  measurement in `docs/performance.md` needs.
- Every endpoint, sender and receiver, builds its transport through the same `Congestion` value,
  so the two commands cannot drift apart.
