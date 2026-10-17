# sluice-live

**Owns:** the segmented append-only log for content that is still being written. Segment sealing, the
`prev_entry_hash` chain, signature verification, tail handling, and the transition to a static root at
close. Spec: `SPEC.md` section 6.

**Must never:** touch the network, the filesystem, or the clock. This crate is sans-IO. It takes bytes
and timer ticks in, and returns actions out. No `tokio` dependency, ever.

**Tests:** `tests/` for the state machine, `proptest` for chain algebra, `fuzz/` for frame parsing, and
the `live/` vectors under `tests/vectors/`.
