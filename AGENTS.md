# Working in this repository

Read this before changing anything. It applies to people and to coding agents alike.

## Ground rules

1. **`SPEC.md` defines the wire.** If code and spec disagree, the code is wrong. Changing the wire
   means editing `SPEC.md` and adding test vectors in the same change.
2. **[`docs/roadmap.md`](docs/roadmap.md#not-built) lists what we do not build.** Don't build anything
   on it, and don't add a dependency that duplicates one we already have.
3. **One crate per change where possible.** Read the crate's README first. It says what the crate owns
   and what it must never do. Don't reach across a crate boundary to make something easier.
4. **Tests decide whether a change is done.** Protocol work needs property tests. Parsing work needs a
   fuzz target. Concurrency work needs a `turmoil` test with its seed.

## Style, enforced by CI

- `#![forbid(unsafe_code)]` everywhere except `sluice-core` and the FFI crates.
- `thiserror` in libraries, `anyhow` only in binaries.
- Six error variants, no more: `Network`, `Verify`, `Storage`, `Policy`, `Cancelled`, `Bug`.
- No async traits, no generic public APIs, no macro tricks. Concrete types and enum dispatch.
- One actor per transfer: owned state and message passing, no shared `Mutex<State>`.
- Functions under 60 lines in the sans-IO crates. Files under 400 lines everywhere.
- Every public item in a library crate has a doc comment with an example. Binaries are exempt.
- `tracing` spans carry the transfer id. Filenames and tickets are never logged at the default level.
- Prose follows [`.claude/skills/writing-style/SKILL.md`](.claude/skills/writing-style/SKILL.md).

## Performance rules

- File bytes never cross a language boundary and never travel over an API.
- No polling loops. An idle process has zero wakeups.
- Nothing is initialized before it is needed. Cold start to a printed ticket is under 30 ms.
- User files are read in place. They are copied into the store only when asked.

## Hand-written and hand-reviewed

These three areas are written and reviewed line by line by a person, because a silent error in any of
them corrupts data or memory:

- the fsync barrier and resume logic
- the verified-range trust boundary
- the C ABI in `sluice-ffi`

## Before opening a change

```
cargo nextest run
cargo clippy --all-targets -- -D warnings
cargo deny check
buf lint proto && buf breaking proto --against '.git#tag=<last-release>'
```
