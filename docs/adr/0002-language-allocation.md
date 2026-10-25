# 0002. Rust for the engine, TypeScript for the UI, nothing else in the product

**Status:** Accepted **Date:** 2026-10-19 **Approved by:** @sanamhub

## Context

The engine depends on iroh, which is Rust (ADR 0001). The UI has to run in a browser tab and in a
desktop shell. Every additional language in the product adds a toolchain that contributors must
install and that CI must build on three operating systems.

An optional multi-user service with policy and audit was considered for the first releases. It would
have been a third language and a second deployable unit before anyone has used the CLI.

## Decision

The product is two languages:

- Rust for the engine, the CLI, the bindings layer and the WASM build.
- TypeScript for the one UI.

Bindings for other languages are generated (ADR 0017), so Python, Go, Swift and the rest do not add
code a maintainer writes by hand.

The multi-user service is not scheduled. It can return as its own proposal once there is demand.

Rejected: Go for the engine. quic-go has no equivalent of iroh's traversal and verified store, and
every Go transfer tool we compared sits below the speed tier we are aiming for.

## Consequences

- `cargo nextest run` works on a clean clone with only a Rust toolchain.
- The UI needs Node for development and nothing else.
- Remote control and multi-user policy wait for the control API (1.1) and for the separate proposal.
- Revisit if the control API is not enough for team deployments that ask for it.
