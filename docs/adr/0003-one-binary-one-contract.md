# 0003. One binary, one control contract, and file bytes stay in Rust

**Status:** Accepted
**Date:** 2026-10-19
**Approved by:** @sanamhub

## Context

Scripts, applications, agents and a UI all need to start transfers and watch progress. If any of
those surfaces carried file content, every one of them would need its own flow control, buffering and
verification, and the engine's throughput would depend on the slowest binding.

Three overlapping API styles (a protobuf socket, gRPC and hand-written REST) were also on the table for
the same operations.

## Decision

- `sluice` is one static binary and is complete on its own.
- `libsluice`, built from the same crates, is the in-process form for bindings.
- `proto/sluice/v1` is the only control API definition, linted and checked for breaking changes by
  `buf`. The engine serves it on a local Unix socket or named pipe, as gRPC and as Connect, so plain
  JSON over HTTP works.
- File bytes never cross a language boundary and never travel over an API. Every surface passes paths,
  tickets and events.
- No HTTP endpoint anywhere accepts file content. The browser is a peer, not an uploader.

## Consequences

- Throughput does not depend on which language started the transfer.
- The control interface stays small: start, cancel, subscribe, query.
- A binding that wants to send a buffer from memory writes it to a file or a pipe first.
- Revisit only with a measured use case that a path or a pipe cannot serve.
