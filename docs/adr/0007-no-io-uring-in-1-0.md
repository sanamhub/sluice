# 0007. No io_uring in 1.0

**Status:** Accepted

## Context

Earlier plans specified io_uring for the disk path. The current Rust ecosystem does not support that
cheaply: `tokio-uring` has had no changelog entry since 2022, `monoio` is effectively unmaintained, and
`compio`, the one actively maintained option, is a complete thread-per-core runtime rather than a drop-in
for tokio.

The arithmetic also argues against it. The 8 Gbit/s target is 1 GB/s, while an NVMe sustains 3 to
7 GB/s. Disk is not the bottleneck. QUIC packet processing is.

## Decision

Use positioned reads and writes on a blocking pool, with sparse files and `fallocate`. Revisit `compio`
only when a profile shows disk in the critical path.

## Consequences

- One less hard dependency and one less hard piece of Rust for a solo maintainer.
- If a future profile does show disk pressure, the store sits behind a port, so the change is contained.
- `sendfile` remains unavailable regardless, because QUIC encrypts in userspace. Zero-copy claims stop at
  avoiding extra copies of our own.
