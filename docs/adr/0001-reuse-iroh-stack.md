# 0001. Reuse iroh, noq and iroh-blobs for transport and storage

**Status:** Accepted **Date:** 2026-10-19 **Approved by:** @sanamhub

## Context

A transfer tool needs NAT traversal, a relay fallback, QUIC, a verified streaming tree, range sets,
a content-addressed store with resume, and a downloader that can use several providers. The iroh
project ships all of them under Apache-2.0 or MIT: `iroh` and `noq` for connectivity and QUIC,
`bao-tree` and `range-collections` for the tree and ranges, `iroh-blobs` for the store and
downloader, `iroh-relay` for the servers.

Writing the tree, the store and the downloader ourselves would take an estimated six weeks. It would
also put verified streaming and store durability, the two places a transfer tool corrupts data, into
new code.

## Decision

Reuse those crates. Use the `iroh-blobs` protocol unchanged for static content. Everything Sluice
adds lives on its own ALPN, `sluice/1`.

Build only what does not exist: live transfer, delta hints, manifests and packing, the parallel data
path, the session layer, and the clients.

Rejected: rust-libp2p. Its hole punching measured 70% success across 4.4M attempts ([IMC
'26](https://arxiv.org/abs/2510.27500)), no better than iroh, and it has no verified-streaming
store.

## Consequences

- `sendme` and DashBeam can fetch Sluice's static tickets.
- Six dependencies come from one company, n0. Everything is permissively licensed, iroh's wire is
  stable since 1.0, and the small crates can be vendored in days. Forking `iroh-blobs` would take
  about a month.
- `iroh-blobs` is pre-1.0 and may change its protocol again. Our ticket envelope turns that into a
  version bump instead of broken tickets, and the interop matrix catches it.
- `noq` has no published throughput numbers. ADR 0010 measures the transport before anything depends
  on it.
- Revisit if n0 stops maintaining `iroh-blobs`, or if the measurement shows the stack cannot reach
  the targets even with ADR 0016.
