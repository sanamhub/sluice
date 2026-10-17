# sluice-wasm

**Owns:** the browser IO shell over the same protocol crates the native build uses. One transport only:
a WebSocket connection to a relay. iroh in a tab has no UDP socket and no WebRTC transport, so there is
no direct path and no hole punching here. Traffic stays end to end encrypted and the relay cannot read
it, but every browser byte passes through it.

**Must never:** reimplement verification logic in TypeScript. `sluice-live` and `sluice-delta` compile
to WASM unchanged, and that is the point of them being sans-IO. Never imply a direct browser path in
the UI, and never report a browser throughput number without naming the relay it went through.

**Must never:** enable `blake3/rayon` or any other thread-using feature. Threads need
`SharedArrayBuffer`, which needs cross-origin isolation, which limits hosting to servers that set COOP
and COEP. Hashing is not the bottleneck on a relayed path, so that trade is not ours to make. ADR 0014.

**Note:** budget is 1.5 MB brotli, loaded lazily when a transfer starts, never at page load. This build
uses the `ring` rustls provider, so its key exchange is classical X25519, not the `X25519MLKEM768` the
native build negotiates. The session panel says which one is in use.

**Note:** hashing is single-threaded `blake3/wasm32_simd`, which has no runtime feature detection, so
the build assumes WASM SIMD: Chrome 91+, Firefox 89+, Safari 16.4+. Firefox and Safari have no
local-disk picker, so those write through OPFS with the quota ceiling surfaced in the UI.

**Tests:** `wasm-bindgen-test` in headless browsers, plus a browser-to-native integration job that runs
against a real relay container.
