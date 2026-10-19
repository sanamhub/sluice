# 0012. The TLS crypto provider is chosen explicitly

**Status:** Superseded by ADR 0018
**Date:** 2026-10-19
**Approved by:** @sanamhub

## Context

Hybrid post-quantum key exchange, `X25519MLKEM768` (RFC 10024), is a property of the rustls crypto
provider, not of rustls. The `aws-lc-rs` provider offers it by default; `ring` has no ML-KEM at all.

Which provider ends up in the build is decided by Cargo feature unification. `noq` selects `aws-lc-rs`
only when `ring` is not also enabled, and iroh's presets prefer `ring` when both are. One transitive
dependency enabling `ring` would therefore remove post-quantum key exchange from every connection,
with no error and no log line.

`aws-lc-rs` contains C and assembly and does not build for WASM.

## Decision

- The native build passes an `aws-lc-rs` provider explicitly through
  `EndpointBuilder::crypto_provider()`.
- A test asserts the negotiated group by reading `negotiated_key_exchange_group` from the connection's
  handshake data. CI also checks `cargo tree` for `ring` in the native build.
- The WASM build uses `ring` and classical X25519, as a documented second profile.
- The session panel and `sluice doctor` show the group each session negotiated.

## Consequences

- Native sessions are post-quantum for confidentiality. Browser sessions are not. Identity is Ed25519
  everywhere, so it is not post-quantum.
- Two providers mean two dependency trees to audit.
- If a WASM-compatible ML-KEM provider appears, the browser profile changes by swapping one crate.
