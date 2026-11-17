# A post-quantum handshake in the browser build: the spike

One question, with numbers: can `sluice-wasm` negotiate `X25519MLKEM768` in a browser, and what does
it cost?

`docs/browser.md` states the state before this: a tab offers `X25519` alone, because `ring` has no
ML-KEM group, and a native peer that applies the rule of `SPEC.md` section 3 closes such a connection.
This folder and the two crates beside it answer whether that can change.

## What is here

- `crates/sluice-wasm/src/pq.rs`: `ring`'s X25519 and RustCrypto's `ml-kem` 0.3.4, combined the way
  rustls's own `X25519MLKEM768` combines them, offered first, with a `Probe` that records the group a
  handshake actually agreed on.
- `crates/sluice-wasm-spike`: the native half. An endpoint with the workspace's `aws-lc-rs` provider,
  every group wrapped in the same probe, serving a blob and answering one byte on `spike/pq/1`. Its
  integration test runs the tab's combiner against `aws-lc-rs` on loopback, with no browser.
- This folder: the runner. `node.mjs` runs the module in Node; `page.html` and `serve.mjs` run it in a
  browser, which posts its report back to the server.

## Building

The wasm build needs a C compiler for `ring`, which a toolchain without `clang` does not have:

```sh
cargo build -p sluice-wasm --release --target wasm32-unknown-unknown
wasm-bindgen --target nodejs --out-dir crates/sluice-wasm/spike/out/node \
  "${CARGO_TARGET_DIR:-target}/wasm32-unknown-unknown/release/sluice_wasm.wasm"
wasm-bindgen --target web --out-dir crates/sluice-wasm/spike/out/web \
  "${CARGO_TARGET_DIR:-target}/wasm32-unknown-unknown/release/sluice_wasm.wasm"
cargo build -p sluice-wasm-spike
```

`wasm-bindgen` has to be the version the crate pins, 0.2.129:
`cargo install wasm-bindgen-cli --version 0.2.129 --locked`.

## Running it

Two processes carry the measurements: a relay, and the native peer.

```sh
iroh-relay --dev &
pq-spike --relay http://localhost:3340/ --bytes 268435456 --config config.json &
```

`pq-spike` prints and writes one JSON line: the relay, its endpoint id, the blob's root and its size.
`localhost` and not `127.0.0.1` on Windows, where the dev relay listens on the IPv6 loopback only.

In Node:

```sh
ROUNDS=50 FETCHES=5 node crates/sluice-wasm/spike/node.mjs config.json
```

In a browser, serve the folder and open it:

```sh
node crates/sluice-wasm/spike/serve.mjs --config config.json   # http://127.0.0.1:8811/
```

The page runs the same measurements and POSTs one JSON report; `serve.mjs` prints it and exits. Any
browser that can open a page works, and the page is the only driver.

## What is measured

- The handshake: `connect` on the spike's ALPN, timed inside the module, median of 50 after two that
  are not counted, once with `X25519` alone and once with the hybrid group first.
- The transfer: the download of one blob, timed inside the module from the first request to the last
  verified byte, median of 5, so neither number includes binding an endpoint or waiting for a relay.
- The module: `sluice_wasm.wasm`, raw and brotli.
- Memory: a tab exposes no counter for a wasm module's linear memory, so the page reports the JS heap
  if the browser has `performance.memory`, and the Node run reports its resident size.
