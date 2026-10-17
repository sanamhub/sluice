# 0002. Rust data plane, C# control plane, TypeScript UI, no Go

**Status:** Accepted

## Context

The maintainer is a C# expert learning Rust, so the temptation is to write everything in C#. .NET has
QUIC via msquic, but no NAT traversal, no relay mesh, and no verified-streaming store, so a C# core means
reimplementing iroh and iroh-blobs. See ADR 0001.

Go was considered. quic-go trails quinn and msquic, and Go's GC and missing SIMD intrinsics hurt the hash
and copy path. Every Go competitor sits in the "correct but not fast" tier.

## Decision

Rust for the data plane. C# .NET 10 Native AOT for the optional control plane, where the official MCP SDK
and gRPC-on-AOT are strongest and where none of the code is hot path. TypeScript for the one UI, because
it is the only language that runs in both a browser and a Tauri webview. No Go, no Dart.

## Consequences

- About half the project is Rust, and roughly a sixth of that is hard async. The rest is sans-IO logic,
  declarative wiring, and adapters.
- Two toolchains cost something at CI setup and release, and buy a control plane in the maintainer's
  fastest language.
- A Go SDK still ships, generated from the `.proto`, without a Go runtime in the product.
