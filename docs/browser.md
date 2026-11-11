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

1. With `tls-ring`, the build stops inside `ring` 0.17.14, which compiles C for the target. A
   toolchain without `clang` fails with `ToolNotFound: failed to find tool "clang"`.
2. Without any TLS feature, `iroh` 1.4 and `iroh-blobs` 0.103.1 build for the target, and the module
   loads and runs in Node. Binding an endpoint then fails at run time, and the message names why:

```
cannot open an endpoint: Missing or incompatible rustls crypto provider configured
```

The provider is not optional: `iroh`'s own documentation says the crypto provider is the only
mandatory option on the endpoint builder, and its `Minimal` and `N0` presets set it from whichever
TLS feature is enabled (`iroh-1.4.1/src/endpoint/presets.rs`). So the count that builds and the count
that runs are different counts, and a browser endpoint needs `ring` after all.

**The key exchange is classical.** `ring` has no ML-KEM group, so a tab negotiates `X25519` and
nothing else. A native peer applies the rule of `SPEC.md` section 3 and closes such a connection with
the policy code before it serves or sends anything, so a browser session works only against a peer
that does not apply it. That refusal is correct and is not weakened for the browser: the native build
keeps requiring `X25519MLKEM768` of every peer, and the browser side stays a proof until a
post-quantum provider that builds for WASM exists.

`zig cc` is the stand-in for `clang` on a toolchain without LLVM, with the freestanding wasm target,
and it first stops at the same place:

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
URL and a bad one, and `endpoint_id` returns 64 hex characters. Without a provider it failed with the
message above; with `ring` and the flags the next section gives, it binds a real endpoint. The parts
that need no endpoint are sound and tested: the relay URL in the form `iroh` dials with, the
relay-only mode, the memory store, and the root parser.

Building the whole workspace for the target does not work and is not meant to: the native crates ask
for `aws-lc-rs`, which has no wasm build, and cargo unifies a package's features across one build.

## One thing the first step found

`iroh_blobs::Hash`'s parser asserts on a string whose length is not 64 rather than answering with an
error. A page that hands a tab a pasted root must not be able to stop it with a short string, so
`sluice_wasm::hash` checks the length and the characters before the parser sees them, and a test
holds that.

## The first proof, made

The first step is done, and it ran in a browser: a headless Chromium loaded a page from a local
static server, the page opened an endpoint that reaches the network only through a relay, and it
fetched content from a native `sluice send` through that relay.

What ran: `iroh-relay --dev` with `http_bind_addr = "0.0.0.0:3340"` and metrics off; a native
`sluice send payload.bin --relay http://127.0.0.1:3340/` serving a 2 MiB file of fixed bytes; and a
page that calls `fetch_blob` twice. The first call fetches the ticket's root, which is the 64-byte
hash sequence of the manifest: the manifest root and the file's root. The page takes the file root
out of those bytes and fetches it too.

```
sequence 64 bytes
file root e325b507062d2c94ca342bdb8bba07506e28cf478d99f8a81210393ea0dc7a78
file 2097152 bytes, sha256 dac7494485cf5bce3989b90f0135d5cc2f68ae532a3e9b7fad4b9ce2f4dba88c, 1654 ms
```

The SHA-256 of the file the sender served is `dac7494485cf5bce3989b90f0135d5cc2f68ae532a3e9b7fad4b9ce2f4dba88c`,
so the bytes in the tab are the bytes on the sender's disk. Every block was verified against the
file's BLAKE3 root by `iroh-blobs` on the way in, and the bytes only reach the page after that. The
sender's own log says what happened: `receiver … connected through a relay`.

## What made it work

The first attempt failed with `Unable to download <root>`, in a browser as well as in Node, and the
cause was the peer's address. A tab can reach nothing directly, and `presets::Minimal` configures no
address lookup, so the endpoint had an id and no way to dial it. The ticket already says which relay
the sender is on, so `fetch_blob` now hands that address to the endpoint:

```rust
let lookup = MemoryLookup::new();
lookup.add_endpoint_info(EndpointAddr::new(sender).with_relay_url(url.clone()));
```

That is the whole fix, and it is the difference between a browser that can fetch and one that cannot.

## What is not done

- The page is a test page, not the app. The screens, the Svelte 5 UI, the PWA, the service worker,
  the content security policy and the budgets are all unwritten.
- Receiving writes nothing yet: the 2 MiB sat in the tab's memory. Streaming to disk through the
  File System Access API or OPFS, resume, and the manifest limits are the next part.
- Sending from a tab is not started.
- Nothing is tested by Vitest or Playwright in CI; the run above was driven by hand.

## Two harness notes

A `sluice send -` read from a pipe needs the writer to close it. A shell that keeps the pipe open
until the child exits never does, and the send waits for ever; on Windows PowerShell that is the
default. Redirect a file instead, with `cmd /c "sluice send - < payload.bin"`, or run the same
redirect from a shell that closes the pipe when the file ends.

`--dump-dom` prints the page while a fetch is still in flight, so read the result from the live page
instead: start the browser with `--remote-debugging-port=9222`, and evaluate
`document.body.innerText` over the DevTools protocol once the page has reported its digest.