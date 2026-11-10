# The browser build

What a browser build of Sluice needs, what the first step of it established, and what it has not
done yet. The design is ADR 0009 (one Svelte 5 UI), ADR 0011 (a browser endpoint is always relayed)
and ADR 0014 (one hashing thread and no special response headers).

## What is settled

- A tab reaches peers through a relay and never directly. A relay URL is part of every ticket the
  browser build serves or fetches.
- The wasm build hashes with `blake3` through the module in `bench/wasm-hash`, the same code the
  native build uses for the same roots.
- The page needs no special response headers, so it works from any static host.

## The dependency set

The native binary asks `iroh` for `tls-aws-lc-rs`, and that provider has no build for
`wasm32-unknown-unknown`. The obvious reading is that a tab needs the other provider, `tls-ring`,
which ADR 0014 names. That reading is wrong, and measuring it is what the first step of this part
did.

With `iroh`'s `tls-ring` feature, the build stops at `ring`'s C compiler: this machine has no
`clang`, so `cc` fails with `ToolNotFound: failed to find tool "clang"`. A wasm-capable clang makes
that path build, and it is still the wrong path: a tab reaches its relay over the WebSocket the
browser already has, so the wasm build needs no Rust TLS provider at all. With
`default-features = false` and no TLS feature, `iroh` 1.4 and `iroh-blobs` 0.103.1 build for the
target on this machine, and they also build for the host, which is what lets the crate be checked by
the gates.

So `crates/sluice-wasm` declares its own `iroh` and `iroh-blobs`, not the workspace's, and the two
commands are:

```sh
cargo check -p sluice-wasm                                     # the host, as the gates do
cargo build -p sluice-wasm --target wasm32-unknown-unknown     # a tab
```

Building the whole workspace for the target does not work and is not meant to: the native crates ask
for `aws-lc-rs`, which has no wasm build, and cargo unifies a package's features across one build.

## One thing the first step found

`iroh_blobs::Hash`'s parser asserts on a string whose length is not 64 rather than answering with an
error. A page that hands a tab a pasted root must not be able to stop it with a short string, so
`sluice_wasm::hash` checks the length and the characters before the parser sees them, and a test
holds that.

## What the first proof is

Part F's first step, and the thing to do before building anything on top of it:

1. a relay on this machine, started the way the probe's tests start one, and a native `sluice send`
   of one small file through it
2. a page that opens an endpoint in the tab, fetches that one blob, and verifies it against the root
   in its ticket
3. the same page loaded in a headless browser, with the ticket passed in and the verified result
   checked by the driver

That run has not happened. The crate has no wasm-bindgen export, no page and no fetch: what it
establishes is the dependency set and the two build commands, and the finding above is what the next
attempt starts from.
