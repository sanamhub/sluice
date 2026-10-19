# 0016. Shard large transfers across connections, with a TCP bulk path for direct peers

**Status:** Accepted
**Date:** 2026-10-19
**Approved by:** @sanamhub
**Relates to:** ADR 0010, ADR 0015

## Context

One QUIC connection is processed by roughly one core. Published measurements put a single userspace
QUIC connection below TCP at 10 Gbit/s: TCP beat six implementations by 16% to 297%, and only a stack
using both GSO and GRO reached 8.3 Gbit/s. Kernel QUIC on a 100G NIC reached 7.48 Gbit/s at 9000 MTU,
against 8.90 for kTLS on the same machine.

The fastest bulk tools (WDT, GridFTP, mscp, rclone with multi-thread streams) all move data over many
streams at once.

## Decision

1. **Sharding is the normal data path.** A transfer above a size threshold splits its ranges across
   `min(cores / 2, 4)` QUIC connections to the same peer. The threshold is set by M0.
2. **A TCP bulk path is built if M0 asks for it.** When a direct route to the peer exists, data moves
   over several TCP streams with TLS 1.3 (kTLS on Linux), plugged in through iroh's custom transport
   hook. QUIC keeps discovery, hole punching, relay and control. A relayed path never uses it.

Both are ours to write, in Rust. Neither needs a fork of iroh or `noq`.

Rejected: replacing `noq` with `msquic`. It has the best published feature set (XDP, RSS, segmentation
offload), but it is C behind FFI and has no traversal or relay.

Rejected: UDT, KCP and Tsunami-style protocols. They are unmaintained or break down when bandwidth is
limited.

## Consequences

- The 10 GbE target becomes reachable without waiting for upstream QUIC performance work.
- The TCP path needs its own handshake bound to the QUIC session's identity, and its own tests. That
  is real work, and the M0 result decides whether it lands in 1.0 or 1.1.
- Verification is unchanged: bytes arriving over TCP are checked against the same BLAKE3 tree.
- Revisit the TCP path if `noq` reaches the target on its own, through ACK frequency and receive-side
  batching.
