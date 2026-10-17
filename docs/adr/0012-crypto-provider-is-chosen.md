# 0012. The rustls provider is chosen explicitly, and the browser is a second crypto profile

**Status:** Accepted

## Context

The plan claimed TLS 1.3 with hybrid post-quantum key exchange, `X25519MLKEM768`, citing that rustls
carries ML-KEM in-crate since 0.23.22. That sentence is true and the conclusion drawn from it was not.

ML-KEM is implemented only by the `aws-lc-rs` provider, and preferred only with the
`prefer-post-quantum` feature, which that provider enables by default. The `ring` provider has no ML-KEM
at all. So the property depends entirely on which provider ends up in the tree, and that had been left
to whatever `noq` happens to default to.

`aws-lc-rs` carries C and assembly and is not a practical WebAssembly target, so the browser build
cannot have the same answer as the native build no matter what we prefer.

## Decision

- The native build selects `aws-lc-rs` explicitly rather than inheriting a default. M0 checks whether
  `noq` accepts an injected provider; if it does not, that is a patch we send upstream.
- M0 also asserts the negotiated group in a test. A crypto property that is not asserted is a hope.
- `sluice-wasm` uses `ring` and negotiates classical X25519. This is a documented second profile, not a
  regression to fix.
- The session panel and `sluice doctor` name the negotiated group per session. One marketing claim never
  covers both profiles.

## Consequences

- Confidentiality is post-quantum on native paths, classical in the browser, and identity is Ed25519 and
  post-quantum on neither. `SECURITY.md` says exactly that.
- Two providers means two dependency trees and two audit surfaces, which is the price of a browser client.
- If a WASM-friendly ML-KEM provider appears, the browser profile changes by swapping one crate.
