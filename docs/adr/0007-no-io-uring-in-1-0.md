# 0007. No io_uring in 1.0

**Status:** Accepted
**Date:** 2026-10-19
**Approved by:** @sanamhub

## Context

At 10 Gbit/s the disk has to sustain 1.25 GB/s. A PCIe 4 NVMe drive sustains 3 to 7 GB/s, and
published QUIC measurements put the bottleneck in userspace packet processing well before that
(`docs/performance.md`).

In Rust, `tokio-uring` is dormant and `glommio` and `monoio` are not actively maintained. `compio` is
maintained and has a production user in Apache Iggy, but it is a complete thread-per-core runtime, not
an addition to tokio.

## Decision

Use positioned reads and writes on a blocking thread pool, with sparse files and `fallocate`. Revisit
`compio` when a profile shows disk in the critical path, most likely above 40 Gbit/s or on a slow
filesystem.

## Consequences

- One less runtime to depend on and to learn.
- The store sits behind its own interface, so a later move to `compio` is contained.
- `sendfile` is not available either way, because QUIC encrypts in userspace.
