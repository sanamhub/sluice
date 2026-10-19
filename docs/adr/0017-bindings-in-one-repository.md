# 0017. Bindings for every language, generated, in this repository

**Status:** Accepted
**Date:** 2026-10-19
**Approved by:** @sanamhub
**Relates to:** ADR 0003

## Context

Sluice should be usable from most languages people write applications in. Hand-writing and maintaining
a binding per language is out of reach for a small team. Separate repositories per language split the
issue tracker, the CI and the release process.

The tools differ in what they cover:

- UniFFI generates Kotlin, Swift, Python and Ruby from one definition, with async and callbacks.
  NordSecurity maintains generators for Go and C#.
- napi-rs is the established path for Node.
- wasm-bindgen is the path for the browser.

Two package managers constrain a single repository:

- Swift Package Manager expects `Package.swift` at the repository root.
- Go module tags must carry the module's directory, and `go get` cannot build Rust, so prebuilt
  libraries have to ship with the module.

## Decision

Two binding surfaces, both under `sdk/` in this repository:

1. **In process.**
   - `sluice-ffi`, a hand-written C ABI with a `cbindgen` header, for C and C++.
   - `sluice-uniffi`, a UniFFI facade over it, for Python, Kotlin, Swift, Ruby, Go and C#.
   - `sluice-node` with napi-rs.
   - `sluice-wasm` with wasm-bindgen.
2. **Out of process.** Clients generated from `proto/sluice/v1` for every other language, talking to a
   running daemon.

Following ADR 0003, every binding passes paths, tickets and events, never file content.

The layout:

- `Package.swift` sits at the root and points into `sdk/swift`.
- Go tags take the form `sdk/go/vX.Y.Z`, and the Go module ships prebuilt static libraries.
- All bindings share the core's version number.
- Each one builds in CI only when its folder, the facade crate or `proto/` changes.

## Consequences

- Python, Node, Swift and Kotlin ship first (1.1), then Go and C# (1.2), then generated clients.
- `sluice-ffi` stays hand-written and hand-reviewed. The UniFFI facade lives in its own crate, so
  generated code never mixes with it.
- Third-party UniFFI generators can lag the core UniFFI version. A binding that falls behind is held
  back, not allowed to block a release.
- Revisit separate repositories for Swift or Go if their packaging constraints cost more than one
  repository saves.
