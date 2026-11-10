# sluice-wasm

The browser half of Sluice. A tab reaches a peer through a relay and never directly (ADR 0011), so
this crate starts with the two things every later step needs: a relay URL in the form `iroh` dials
with, and a store to fetch into.

## What is here

- `relay`: a relay URL, parsed, with a refusal that does not quote it.
- `relay_only`: the relay mode a browser build uses, and the only one it uses.
- `store`: the memory store a tab fetches into.
- `hash`: a BLAKE3 root from the 64 hex characters a ticket or a manifest carries.

## The provider, and how the target build gets one

**The dependency set.** The native binary asks `iroh` for `tls-aws-lc-rs`, which cannot build for
`wasm32-unknown-unknown`. A tab needs the other provider, `ring`, through `tls-ring`: `iroh`'s
endpoint builder accepts no endpoint without a crypto provider, and `ring` is the only one that can
be built for the target. This crate declares its own `iroh` with `features = ["tls-ring"]`, separately
from the workspace's, and unbundling it is not possible.

**The C compiler.** `ring` compiles C for the target, so the wasm build needs a wasm-capable C
compiler. LLVM's `clang` is what the CI runners have. A machine without one can point `cc` at
`zig cc` with the freestanding wasm target, and `ring` builds there because its build script drops
the C library headers when the compiler is clang-like:

```sh
export CC_wasm32_unknown_unknown="zig cc -target wasm32-freestanding -nostdlibinc -O2"
export AR_wasm32_unknown_unknown="zig ar"
CFLAGS_wasm32_unknown_unknown="-DRING_CORE_NOSTDLIBINC=1" \
  cargo build -p sluice-wasm --target wasm32-unknown-unknown
```

`-O2` has to come last: the debug profile's `-O0` would re-enable the checks whose runtime the link
does not have.

**The build command.** The workspace is built for the host by the gates, and this crate is built for
the target by hand:

```sh
cargo check -p sluice-wasm                                     # the host, as the gates do
rustup target add wasm32-unknown-unknown
cargo build -p sluice-wasm --target wasm32-unknown-unknown     # a tab
```

Building the whole workspace for the target does not work and is not meant to: the native crates ask
for `aws-lc-rs`, which has no wasm build.

## What is here again

Three wasm-bindgen exports: `relay_reads`, `endpoint_id` and `fetch_blob`. The second binds a real endpoint through
a relay, and it is proved in Node: the module loads, the relay URL is read, and an endpoint id comes
back. `fetch_blob` fetched a 2 MiB file from a native `sluice send` through a local relay, in a
headless browser, and the SHA-256 of the bytes matched the sender's. `docs/browser.md` records the
run and the address lookup that made it work. There is no app yet.
