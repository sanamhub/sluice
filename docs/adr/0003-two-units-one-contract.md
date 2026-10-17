# 0003. Two shipping units, one contract, and bytes never cross a language boundary

**Status:** Accepted

## Context

An earlier draft put the Rust engine in-process behind FFI under a C# daemon. That made the minimum
install a .NET app, put an ABI on the crash path, and forced every contributor to build both toolchains
to run anything. It also carried three overlapping API surfaces: a protobuf control socket, gRPC, and a
hand-written REST API describing the same operations.

## Decision

Two units. `sluice`, a Rust static binary, is the product and is complete alone. `sluice-hub`, a C#
Native AOT service, is optional and adds multi-user policy, audit, web UI, and remote MCP.

One service definition in `proto/sluice/v1`, governed by `buf`, served as gRPC and Connect. The core
serves it on a local socket; the hub serves the same service over the network with auth added. Every SDK
is generated from it.

Two invariants: file bytes never cross a language boundary, and the browser is a peer rather than an
uploader, so no HTTP endpoint anywhere accepts file content.

## Consequences

- Install is one binary. The hub is opt-in.
- FFI overhead stops being a risk item, because control events cross at tens per second.
- Crash isolation in both directions.
- A Go or Python hub becomes a contribution rather than a fork.
- The cost is one more versioned interface, kept small: start, cancel, subscribe, query.
