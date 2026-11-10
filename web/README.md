# The browser side

Sluice in a tab. The module that does the work is `crates/sluice-wasm`; this directory holds the
pages that use it and, in time, the app.

## What is here

- `proof/`, part F's first proof: a page that fetches a file from a native `sluice send` through a
  local relay and reports what it got, with the recipe to run it. It is a harness and not a test, and
  `proof/README.md` says so.

## What the module does

`crates/sluice-wasm` exports three functions to JavaScript: `relay_reads` for a page checking a relay
field, `endpoint_id` for the address a ticket would name, and `fetch_blob`, which fetches a blob
through a relay and returns the bytes only after every block verified against its root. The crate's
README has the build commands, including the C toolchain `ring` needs for the target and the flags
that stand in for `clang` on a machine without LLVM.

## What is not here

- The app. There is no Vite configuration, no `package.json`, no Biome or Vitest configuration, no
  Svelte component, and no service worker; `web/` holds the proof and this file.
- Receiving into storage. A tab fetches into memory and writes nothing, so the manifest limits, the
  File System Access API or OPFS writes, resume and the clearing of a failed receive are all
  unwritten. The manifest rules those need live in `sluice-core`, which a tab cannot build; ADR 0034
  proposes where they should live instead.
- Sending from a tab.
- A workflow. No job builds, tests or hosts any of this, and no budget exists to enforce.

## Running what is here

The proof's recipe is in `proof/README.md` and covers the crate's wasm build, the relay, a sender, a
static server and a browser. Nothing else in this directory runs.
