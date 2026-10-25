# 0010. Measure the transport before building on it

**Status:** Accepted
**Date:** 2026-10-19
**Approved by:** @sanamhub
**Relates to:** ADR 0015, ADR 0016

## Context

`noq` is n0's fork of quinn and carries iroh's QUIC. It has no published throughput numbers. Two open
iroh issues report it well below link speed:

- [#4286](https://github.com/n0-computer/iroh/issues/4286): single-stream `iroh-blobs` between two
  Macs reached 42.65 MB/s with BBR against a 110 MB/s `iperf3` baseline, and 1.3 to 1.7 MB/s with Cubic.
- [#1943](https://github.com/n0-computer/iroh/issues/1943): about 300 Mbit/s where netcat reached 700,
  with tuned windows.

Published research puts userspace QUIC 16% to 297% behind TCP at 10 Gbit/s. Sluice's headline claim is
speed, and every later phase sits on this layer.

## Decision

The transport is measured before any product code exists, on Linux 10 GbE and on macOS:

1. `iroh` plus `iroh-blobs` with one connection and with several.
2. Each with default and tuned windows, under Cubic and BBRv3.
3. Against `iperf3` (one stream and parallel) and a parallel-TCP tool as the bar.
4. On the LAN, 80 ms with 0.1% loss, and 60 ms at 10 Gbit/s.

The outcomes:

- **Within 10% of the ceiling on LAN:** proceed with QUIC only.
- **Close:** send patches upstream, starting with ACK frequency if `noq` lacks it.
- **Far:** ship the TCP bulk path from ADR 0016 in 1.0.

## Consequences

- The riskiest assumption is tested first, at a cost of about two weeks including the harness.
- The numbers are published whatever they say. No public benchmark of this stack exists, so they are
  useful to others too.
- If no option reaches a target, `docs/performance.md` is revised with the measurement attached.
