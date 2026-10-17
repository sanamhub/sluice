# sluice-ffi

**Owns:** the C ABI (`cdylib` and `staticlib`) for embedders who want the engine in-process rather than
as a child process.

**Must never:** pass file bytes across the boundary, or expose more than about ten functions. The
boundary is coarse on purpose: start, cancel, poll events, query. A per-chunk call across FFI would
undo the entire performance design.

**Note:** one of only two crates where `unsafe` is allowed. Every entry point is fuzzed and hand
reviewed. `cargo-semver-checks` runs on this crate because breaking it breaks compiled dependents.

**Tests:** `fuzz/` on every entry point, plus a C smoke-test harness in `tests/`.
