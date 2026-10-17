# 0014. The browser build is single-threaded WASM SIMD, and requires no cross-origin isolation

**Status:** Proposed

Follows from ADR 0011, which established that every browser transfer is relayed for its whole life.

## Context

Open question 6 asked two things: what BLAKE3 costs in WASM, and whether cross-origin isolation headers
are needed for WASM threads, since that constrains where the PWA can be hosted.

The second half is answerable without measuring anything. WASM threads require `SharedArrayBuffer`,
which since Spectre requires the document to be cross-origin isolated:
`Cross-Origin-Opener-Policy: same-origin` plus `Cross-Origin-Embedder-Policy: require-corp`, with every
cross-origin subresource opting in through CORP or CORS. This holds in Chrome, Firefox and Safari
alike, and mobile Safari is the strictest about which COEP variants it will accept.

The consequence is a hosting constraint, not a code constraint. A static host that cannot set response
headers cannot serve a cross-origin-isolated page. GitHub Pages is the obvious casualty.

`blake3` 1.8.7 splits the two concerns cleanly. `wasm32_simd` is a SIMD implementation with no runtime
detection: the build asserts WASM SIMD rather than probing for it, which is safe, since WASM SIMD is
Chrome 91+, Firefox 89+, Safari 16.4+. Multithreading is a separate feature, `rayon`, and it is the one
that would pull in `SharedArrayBuffer` and therefore the headers.

## Decision

`sluice-wasm` builds with `blake3/wasm32_simd`, **single-threaded**, without `rayon`. The PWA declares
no cross-origin isolation requirement and sets no COOP or COEP headers, so it can be served from any
static host.

The justification is ADR 0011's own finding: every browser transfer is relayed end to end, so the
ceiling on a browser transfer is the relay's bandwidth, not the tab's hash rate. Spending the hosting
constraint to parallelize a stage that is not the bottleneck is the wrong trade.

This decision is **conditional on a measurement that has not been made yet.** M0 publishes BLAKE3 WASM
SIMD throughput in Chrome, Firefox and Safari, on desktop and on mobile Safari. If single-threaded
hashing in the slowest supported browser lands below the relay bandwidth an operator would plausibly
provision, this ADR is superseded rather than quietly ignored, and the PWA's hosting requirements
change with it.

## Consequences

- The PWA hosts anywhere: GitHub Pages, any CDN, a static directory on the rendezvous VPS. No header
  control required, which matters because the operator running the relay is often not running a
  web host.
- Two crypto profiles remain two crypto profiles. This ADR is about parallelism, not about the provider
  split in ADR 0012, and it does not change that `sluice-wasm` uses `ring` and gets classical key
  exchange.
- No public per-browser BLAKE3 WASM throughput figures exist, so the benchmark page is an original
  measurement and gets published like the rest of them.
- If the measurement forces threads later, the cost is not code, it is the hosting requirement and a
  documentation change telling operators their static host must set two headers.
