# The browser build

What a browser build of Sluice needs, what its first step established, and what it has not done yet.
The design is ADR 0009 (one Svelte 5 UI), ADR 0011 (a browser endpoint is always relayed) and ADR
0014 (one hashing thread and no special response headers).

## What is settled

- A tab reaches peers through a relay and never directly. A relay URL is part of every ticket the
  browser build serves or fetches.
- The wasm build hashes with `blake3` through the module in `bench/wasm-hash`, the same code the
  native build uses for the same roots.
- The page needs no special response headers, so it works from any static host.

## The crypto provider is the blocker, and it needs a C compiler

The native binary asks `iroh` for `tls-aws-lc-rs`, and that provider has no build for
`wasm32-unknown-unknown`. `iroh` names exactly two providers, `ring` (its `tls-ring` feature) and
`aws-lc-rs`, and on this target only `ring` can be the one.

Two measurements, in the order they were made:

1. With `tls-ring`, the build stops inside `ring` 0.17.14, which compiles C for the target. This
   machine has no `clang`, and `cc` fails with `ToolNotFound: failed to find tool "clang"`.
2. Without any TLS feature, `iroh` 1.4 and `iroh-blobs` 0.103.1 build for the target, and the module
   loads and runs in Node. Binding an endpoint then fails at run time, and the message names why:

```
cannot open an endpoint: Missing or incompatible rustls crypto provider configured
```

The provider is not optional: `iroh`'s own documentation says the crypto provider is the only
mandatory option on the endpoint builder, and its `Minimal` and `N0` presets set it from whichever
TLS feature is enabled (`iroh-1.4.1/src/endpoint/presets.rs`). So the count that builds and the count
that runs are different counts, and a browser endpoint needs `ring` after all.

`zig cc` was tried as a stand-in for `clang`, with its freestanding wasm target, because it is the
one C compiler on this machine. It reaches `ring`'s own code and stops there:

```
ring-core/check.h:27:11: fatal error: 'assert.h' file not found
```

A freestanding target ships no C library headers, and `ring` includes `assert.h`. What a browser
build needs is a wasm-capable LLVM `clang` with a sysroot for the target, which the CI runners have
and this machine does not.

## What the crate establishes

`crates/sluice-wasm` is a workspace member with its own `iroh` and `iroh-blobs` dependency specs, and
it builds for the target and for the host:

```sh
cargo check -p sluice-wasm                                     # the host, as the gates do
cargo build -p sluice-wasm --target wasm32-unknown-unknown     # a tab
wasm-bindgen --target nodejs --out-dir out \
  target/wasm32-unknown-unknown/debug/sluice_wasm.wasm          # the JS side
```

That pipeline works end to end: the generated module loads in Node, `relay_reads` answers for a good
URL and a bad one, and `endpoint_id` fails with the provider message above. The parts that need no
endpoint are sound and tested: the relay URL in the form `iroh` dials with, the relay-only mode, the
memory store, and the root parser.

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

Steps 2 and 3 need a working endpoint, and the endpoint needs the C toolchain above. That run has
not happened.
