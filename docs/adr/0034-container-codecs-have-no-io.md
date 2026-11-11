# 0034. The container codecs move into a crate with no IO, for the browser to share

**Status:** Accepted **Date:** 2026-11-11 **Approved by:** @sanamhub **Relates to:** ADR 0008, ADR 0009, ADR 0014

## Context

The first proof is made: a headless browser fetched a 2 MiB file from a native `sluice send`
through a local relay, and the bytes matched the sender's. The next step is a tab that receives
properly, which means the manifest limits and the path rules applied inside the page, and a write
stream that starts while the transfer is still arriving.

Those rules are not in a crate a tab can use. `sluice_core::manifest`, the path rules and the ticket
codec live in `sluice-core`, which brings the filesystem store, `rustls` through `iroh`, and
`getrandom` with it. A wasm build of it does not exist and should not: its whole job includes
opening files and taking a home lock.

The alternative to sharing the codec is writing it twice. Twelve reference spellings, two Unicode
foldings, reserved Windows names, alternate data stream rules and a limit's exact boundary are not
a rule that two copies keep in step: the vectors would pass twice for a while and then diverge on the
line a copy missed. Two copies of a check that decides where a stranger's bytes are written is the
kind of duplication this repository has avoided for the live log and for delta, both of which are
crates with no IO for that reason (ADR 0008).

## Decision

1. **A new crate, `crates/sluice-format`, holds every rule that turns bytes into values and back with
   no IO**: the directory manifest and its root, the path rules and their comparison key, the ticket,
   the receipt, the pack table, the limits, and the hash sequence. It depends on `blake3`,
   `ed25519-dalek`, `serde`, `thiserror` for the error's display and the URL parser, and on nothing
   that opens a file or a socket.
2. **`Error` moves with them.** Every codec answers with `sluice_core::Error`, and that type is six
   variants carrying a string or nothing, with no `io::Error` inside it, so it is a value like the
   rest and not an IO dependency. It is defined once in the format crate and re-exported by the core
   as `sluice_core::Error`, so no caller changes and the six kinds stay one list. The IO paths in the
   core keep using it and map their failures into it as they do now.
3. **`sluice-core` re-exports it**, so `sluice_core::manifest`, `sluice_core::ticket` and the rest keep
   their paths and their documentation, and no caller changes.
4. **`sluice-core` keeps the IO**: the home, the store, the repair, the sweep, the platform file mode
   mapping and the writer that touches the disk. A rule that needs to know how one platform spells a
   file name stays in the core and consults the format crate for the verdict.
5. **The browser crate depends on `sluice-format`** for these rules. It already depends on
   sluice-delta, for the chunker and the matcher a tab uses as a hint, and on nothing else of ours.
   The limits a tab applies are the same numbers, read from the same place, that a native receive
   applies.
6. **The move is a refactor with no wire change.** `cargo xtask conformance`, the vectors and the
   existing tests are the check that nothing moved but the code.

## Consequences

- One copy of every rule that decides what a stranger can write, in a crate that can be tested as a
  function call and compiled for a tab.
- A new workspace member to keep under the size and documentation rules, excluded from `dist` like
  `sluice-live`, `sluice-delta` and the wasm crate.
- A large move: every `use` path inside the core changes, the error type changes home, and the public re-exports have to keep
  their docs and their examples. The conformance run and the doc tests are what says the move was
  faithful, and it lands as its own commit so the diff can be read as a move rather than a rewrite.
- The core's manifest module becomes a thin re-export, which is one more hop for a reader to follow;
  the crate's documentation says why the rule lives where it does.
- Revisit this if a rule turns out to need the filesystem to decide, or if the browser build stops
  needing the manifest at all.
