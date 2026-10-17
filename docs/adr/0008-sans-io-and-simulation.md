# 0008. Sans-IO protocol crates plus deterministic simulation testing

**Status:** Accepted

## Context

Every transfer bug worth finding is a race under loss or a crash at the wrong moment. Tests that depend
on real sockets and real sleeps find those bugs by luck, and then cannot reproduce them.

## Decision

The two protocol crates we own, `sluice-live` and `sluice-delta`, perform no IO. They take bytes and
timer ticks and return actions, following the `quinn-proto` pattern. Resume, durability, and scheduling
are tested with `turmoil` 0.7, which simulates the network plus filesystem durability and torn writes,
and prints a reproducing seed on every failure.

Scope is limited to what we own. quinn, noq, and iroh test their own transport, so re-simulating QUIC
would be theatre.

## Consequences

- A protocol test is a pure function call, and fuzzing takes a byte slice.
- The WASM build reuses these crates unchanged, so verification logic exists once rather than twice.
- Generated code becomes reviewable, because a failing seed reproduces exactly on any machine.
- This is also the easiest Rust in the repository to write and review, which matters for a solo
  maintainer.
