# Working in this repository

For both humans and coding agents. Read this before generating anything.

## Ground rules

1. **`SPEC.md` is the source of truth for the wire.** Code follows it. If code and spec disagree, the
   spec wins and the code is a bug. Changing the wire means editing `SPEC.md` and adding test vectors
   in the same change.
2. **`plan.md` section 3 lists what we deliberately do not build.** Do not implement anything on that
   list. Do not add a dependency that duplicates one we already have.
3. **One crate per task.** Read that crate's README first. It states what the crate owns and what it
   must never do. Do not reach across a crate boundary to make something easier.
4. **Tests are the acceptance oracle**, not a reviewer's patience. Protocol work needs property tests.
   Parsing work needs a fuzz target. Concurrency work needs a `turmoil` seed.

## Style, enforced by CI

- `#![forbid(unsafe_code)]` outside `sluice-core` and `sluice-ffi`
- `thiserror` in libraries, `anyhow` at binary edges
- Error taxonomy is exactly six variants: `Network`, `Verify`, `Storage`, `Policy`, `Cancelled`, `Bug`
- No async traits, no generic public APIs, no macro magic. Concrete types and enum dispatch
- Actor per transfer: owned state, message passing, no shared `Mutex<State>`
- Functions under 60 lines in `sluice-live` and `sluice-delta`, files under 400 lines everywhere
- Public items carry a doc comment with an example
- `tracing` spans keyed by transfer id. Never log a filename or ticket at default level
- Prose follows the `writing-style` skill in `.claude/skills/writing-style/SKILL.md`

## Performance rules that are not negotiable

- File bytes never cross a language boundary and never traverse an API
- No polling loops. Idle means zero wakeups
- Nothing is initialized before it is needed. Cold start budget is 30 ms
- Reference user files in place. Copy into the store only when explicitly asked

## Do not delegate these

Hand-write and hand-review: the fsync barrier and resume logic, the verified-range trust boundary, and
everything in `sluice-ffi`. A silent error in those three places is the kind that ends a project.

## Before opening a change

```
cargo nextest run
cargo clippy --all-targets -- -D warnings
cargo deny check
buf lint proto && buf breaking proto --against '.git#tag=<last-release>'
```
