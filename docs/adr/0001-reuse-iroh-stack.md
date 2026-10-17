# 0001. Reuse iroh, noq, and iroh-blobs instead of building transport and store

**Status:** Accepted

## Context

Earlier drafts planned to build a verified BLAKE3 tree, a range-set library, a content-addressed store
with resume, and a multi-provider downloader. All four already exist as `bao-tree`, `range-collections`,
and `iroh-blobs`, written by the team that also wrote the transport, permissively licensed, and running
in production behind hundreds of millions of endpoint connections a month.

Building them ourselves would take roughly six weeks and would place the two functions where a transfer
tool corrupts data, verified streaming and store durability, into brand-new code.

## Decision

Reuse `iroh` (connectivity), `noq` (QUIC), `iroh-blobs` (store, verified streaming, downloader),
`bao-tree` and `range-collections` (tree and ranges), and `iroh-relay` (as a container). Build only what
nobody else has: live streaming, delta hints, manifests and packing, the session layer, and the clients.

Adopt the `iroh-blobs` protocol unchanged for static content. Our extensions live on ALPN `sluice/1`.

## Consequences

- The code we own is the code nobody else wrote.
- `sendme` and DashBeam can fetch our static tickets, a compatibility win we did not pay for.
- Six dependencies now come from one founder-backed company. Mitigations in `plan.md` section 2.2:
  permissive licenses, wire stability at iroh 1.0, small crates vendorable in days, and a fork plan for
  the one expensive crate.
- `iroh-blobs` is pre-1.0 and may break its ALPN once more. Our ticket envelope contains the blast
  radius, and the interop matrix catches drift.
