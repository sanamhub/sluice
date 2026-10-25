# 0008. Protocol crates do no IO, and concurrency is tested by simulation

**Status:** Accepted **Date:** 2026-10-19 **Approved by:** @sanamhub

## Context

The bugs that matter in a transfer tool are races under packet loss and crashes at the wrong moment.
Tests built on real sockets and real sleeps find those by luck, and a failure seen once often never
reproduces.

## Decision

- The protocol crates we own (`sluice-live`, later `sluice-delta`) perform no IO. They take bytes
  and timer ticks and return actions, the way `quinn-proto` does.
- Resume, durability and scheduling are tested with `turmoil`, which simulates the network and the
  filesystem, injects torn writes, and prints the seed of every failing run.
- The QUIC stack is not simulated. Its own project tests it.

## Consequences

- A protocol test is a function call, and a fuzz target takes a byte slice.
- The WASM build uses the same protocol crates, so verification logic exists once.
- A failing concurrency test reproduces on any machine from its seed.
- `turmoil`'s filesystem simulation is behind an `unstable-fs` feature. Durability tests go through
  one small `Fs` trait in `sluice-core`, so if that feature changes, a hand-written fault-injecting
  implementation replaces it in about a day.
