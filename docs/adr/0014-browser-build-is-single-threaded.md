# 0014. The browser build hashes on one thread and needs no special headers

**Status:** Accepted **Date:** 2026-10-19 **Approved by:** @sanamhub **Relates to:** ADR 0011

## Context

WASM threads need `SharedArrayBuffer`, which browsers expose only to cross-origin isolated pages:
`Cross-Origin-Opener-Policy: same-origin` and `Cross-Origin-Embedder-Policy: require-corp`. A static
host that cannot set response headers, GitHub Pages for example, cannot serve such a page.

`blake3` separates SIMD from threading. The `wasm32_simd` feature needs WASM SIMD (Chrome 91,
Firefox 89, Safari 16.4 or later) and no threads. The `rayon` feature adds threads.

Every browser transfer goes through a relay (ADR 0011), so the relay's bandwidth bounds browser
throughput, not the tab's hash rate.

## Decision

`sluice-wasm` uses `blake3/wasm32_simd` without `rayon`. The PWA sets no COOP or COEP headers and
can be served from any static host.

This holds only if single-threaded hashing outpaces the relay. BLAKE3 WASM throughput is measured in
Chrome, Firefox and Safari, on desktop and on iOS. If the slowest supported browser hashes slower
than a relay an operator would plausibly run, a new ADR replaces this one.

## Consequences

- The PWA can be hosted on GitHub Pages, any CDN, or the rendezvous VPS itself.
- If threads become necessary later, the cost is a hosting requirement and a docs change, not code.
- No public per-browser BLAKE3 WASM figures exist, so the per-browser numbers are published with the
  rest.
