# sluice-wasm

The browser half of Sluice. A tab reaches a peer through a relay and never directly (ADR 0011), so
this crate starts with the two things every later step needs: a relay URL in the form `iroh` dials
with, and a store to fetch into.

## What is here

- `relay`: a relay URL, parsed, with a refusal that does not quote it.
- `relay_only`: the relay mode a browser build uses, and the only one it uses.
- `store`: the memory store a tab fetches into.
- `hash`: a BLAKE3 root from the 64 hex characters a ticket or a manifest carries.

## Two things it settles

**The dependency set.** The native binary asks `iroh` for `tls-aws-lc-rs`, which cannot build for
`wasm32-unknown-unknown`. A tab reaches its relay over the WebSocket the browser already has, so the
wasm build needs no TLS provider at all. With `default-features = false` and no TLS feature, the
pinned `iroh` 1.4 and `iroh-blobs` 0.103.1 build for the target, and this crate's own dependency
spec says so, separately from the workspace's.

**The build command.** The workspace is built for the host by the gates, and this crate is built for
the target by hand:

```sh
cargo check -p sluice-wasm                                     # the host, as the gates do
rustup target add wasm32-unknown-unknown
cargo build -p sluice-wasm --target wasm32-unknown-unknown     # a tab
```

Building the whole workspace for the target does not work and is not meant to: the native crates ask
for `aws-lc-rs`, which has no wasm build.

## What is not here

No wasm-bindgen export, no page, no fetch of a blob, and no browser run. `docs/browser.md` says what
the first proof is and what it found.
