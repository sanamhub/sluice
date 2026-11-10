# The browser build

What a browser build of Sluice needs, and what the first proof of it found on the machine this was
written on. The design is ADR 0009 (one Svelte 5 UI), ADR 0011 (a browser endpoint is always
relayed) and ADR 0014 (one hashing thread and no special response headers).

## What is settled

- A tab reaches peers through a relay and never directly. A relay URL is part of every ticket the
  browser build serves or fetches.
- The wasm build hashes with `blake3` through the module in `bench/wasm-hash`, the same code the
  native build uses for the same roots.
- The page needs no special response headers, so it works from any static host.

## The TLS provider needs a C toolchain

`aws-lc-rs` does not build for `wasm32-unknown-unknown` at all. The provider a browser build can use
is `ring`, which ADR 0014 names, and `ring` compiles C and assembly for the target, so the build
machine needs a wasm-capable C compiler. On the machine this was written on there is none, and the
build stops exactly here:

```
error: failed to run custom build command for `ring v0.17.14`
  warning: ring@0.17.14: Compiler family detection failed due to error: ToolNotFound: failed to find tool "clang": program not found
  process didn't exit successfully: `...\build-script-build` (exit code: 1)
  error occurred in cc-rs: failed to find tool "clang": program not found
```

The same crate builds for the host on that machine without a C compiler, and `bench/wasm-hash`
builds for `wasm32-unknown-unknown` there, so the target and the wasm toolchain are in place; what is
missing is `clang` (and with it `wasm-ld`) for the target. Either of these makes a browser build
possible on this machine:

- install LLVM and put `clang` and `wasm-ld` on the path, which is what the CI runners have
- replace `ring` with a pure-Rust TLS provider that supports the target, of which there is none in
  `iroh` today: its only two providers are `ring` and `aws-lc-rs`

## One question the first proof has to answer

`iroh` chooses its TLS provider with a cargo feature, and the native build of this workspace asks for
`tls-aws-lc-rs`. Cargo unifies features for one package across one build, so a workspace member that
asks for `tls-ring` may end up with both providers, and `aws-lc-rs` does not build for the target at
all. The `fuzz/` crates of this repository are their own workspaces for a neighbouring reason, and the
browser crate may need the same. This has not been tested: the build stops at `ring` long before the
unification matters.

## What the first proof is

Part F's first step, and the thing to do before building anything on top of it:

1. a relay on this machine, started the way the probe's tests start one, and a native `sluice send`
   of one small file through it
2. a page that opens an endpoint in the tab, fetches that one blob, and verifies it against the root
   in its ticket
3. the same page loaded in a headless browser, with the ticket passed in and the verified result
   checked by the driver

That run has not happened. The finding above is the state of the first step on this machine, and it
is recorded here so the next attempt starts from it rather than from the error.
