# 0018. Post-quantum key exchange is checked at the handshake

**Status:** Accepted
**Date:** 2026-10-19
**Approved by:** @sanamhub
**Supersedes:** ADR 0012

## Context

ADR 0012 chose the `aws-lc-rs` provider and planned two guards: a test of the negotiated group, and a
CI check that `ring` is absent from `cargo tree`. Building the transport probe showed that the second
guard cannot hold and the first one is the only one that works.

- **The provider is not enough.** `aws-lc-rs` offers `X25519MLKEM768`, but rustls sends a key share
  for it first only with its `prefer-post-quantum` feature. That feature is on by default, and we turn
  rustls defaults off. Without it the probe's first handshake settled on plain `X25519`, with no error.
  The handshake assertion caught it.
- **`ring` cannot be kept out.** It arrives through `noq-proto`, `rustls-webpki`, and `rcgen` by way of
  `irpc` and `iroh-blobs`. A `cargo tree` check would fail on every build, so it would be switched off
  or ignored. The probe still negotiates `X25519MLKEM768`, because the provider passed to the endpoint
  decides the handshake, not which crates are linked.

## Decision

1. The native build passes the `aws-lc-rs` provider explicitly to the endpoint builder, as in ADR 0012.
2. The workspace `rustls` dependency enables `prefer-post-quantum`, with a comment saying why.
3. Every connection Sluice opens reads `negotiated_key_exchange_group` from the handshake data. If it
   is not `X25519MLKEM768`, the connection is closed with a `Policy` error.
4. A loopback test on Linux, macOS and Windows asserts the group on every CI run. There is no
   `cargo tree` check for `ring`.
5. The browser build keeps the second profile from ADR 0012: `ring` and classical `X25519`. How a
   native peer accepts a classical handshake from a browser peer, and only from one, is decided with the
   browser build. Until then every connection requires `X25519MLKEM768`.
6. The session panel and `sluice doctor` show the negotiated group, as in ADR 0012.

## Consequences

- A silent downgrade, from any cause, becomes a failed connection instead of a weaker one.
- `ring` is compiled into the native binary although it is never used for the handshake. Its size
  counts against the 12 MB binary budget in `docs/performance.md`, and it is one more crate to audit.
- A peer built without `prefer-post-quantum`, or with another provider, cannot connect. That is the
  intended result, and the error says which group was negotiated.
- Revisit item 4 if iroh, `noq` and `irpc` stop depending on `ring`; a `cargo tree` check would then
  be cheap to add back as a second guard.
