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
one C compiler on this machine. It first stopped at the same place:

```
ring-core/check.h:27:11: fatal error: 'assert.h' file not found
```

That one is answerable. `ring`'s build script passes `-nostdlibinc` and defines
`RING_CORE_NOSTDLIBINC` when the compiler is clang-like, and its own headers then skip the C library.
`cc` could not tell that `zig cc` is clang, so those flags never arrived; giving them by hand, and
turning off the checks whose runtime the wasm link does not carry, builds `ring` for the target:

```
CC_wasm32_unknown_unknown="zig cc -target wasm32-freestanding -nostdlibinc -O2"
AR_wasm32_unknown_unknown="zig ar"
CFLAGS_wasm32_unknown_unknown="-DRING_CORE_NOSTDLIBINC=1"
```

`-O2` has to come last, or the debug profile's `-O0` re-enables the checks and the link fails on
`__ubsan_handle_type_mismatch_v1`. With that, `sluice-wasm` builds for `wasm32-unknown-unknown` on a
machine with no LLVM, and the endpoint is real:

```sh
wasm-bindgen --target nodejs --out-dir out \
  target/wasm32-unknown-unknown/debug/sluice_wasm.wasm
node -e 'require("./out/sluice_wasm.js").endpoint_id("http://127.0.0.1:3340/")'
# an endpoint id, 64 hex characters
```

That is the first half of the first proof: an endpoint that reaches the network through a relay,
opened from the wasm module in Node. What it has not done is fetch anything.

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

The endpoint is proved (the command above). The fetch is written and does not work yet: with a relay
on this machine, a native `sluice send` through it and the ticket's parts handed to `fetch_blob`, the
module answers

```
the fetch stopped: Unable to download 278b5293184ca53b6528d057ad59f78b99f94f639b4d8e3dcf0266129d435d94
```

The sender was serving and the same ticket's relay was up, so the failure is in the browser side: the
wasm endpoint's WebSocket to the relay, or the way the peer is looked up from an endpoint id with no
addresses. That is the next thing to settle, and it is a diagnosis rather than a design question.
A page and a headless browser come after it.
