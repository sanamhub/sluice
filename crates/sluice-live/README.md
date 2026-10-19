# sluice-live

**Owns:** the segment log for files that are still being written. Sealing segments, the
`prev_entry_hash` chain, signature checks, the provisional tail, and the switch to a static root on
close. Specified in `SPEC.md` section 6.

**Must never:** touch the network, the filesystem or the clock. Bytes and timer ticks go in, actions
come out, and there is no `tokio` dependency. The browser build uses this crate unchanged.

**Tests:** state machine tests in `tests/`, `proptest` for the chain, a fuzz target for frame parsing,
and the `live/` vectors in `tests/vectors/`.
