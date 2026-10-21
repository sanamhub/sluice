# sluice-live

**Owns:** the segment log for files that are still being written. The four records and their bytes,
signing them, the `prev_entry_hash` chain, the checks a receiver makes on records from untrusted
providers, and the switch to a static root on close. Specified in `SPEC.md` section 6.

**Does not own:** fetching segments and the provisional tail. Those belong to the transfer layer,
which this crate hands the sealed `segment_root` and `len` to re-verify the tail against.

**Must never:** touch the network, the filesystem or the clock. Bytes go in, outcomes come out, and
there is no `tokio` dependency and no dependency on `sluice-core`. The browser build uses this crate
unchanged, and `cargo check -p sluice-live --target wasm32-unknown-unknown` checks that it builds.

**Tests:** a test per rule of section 6 in `tests/`, `proptest` for the chain, the fuzz target `log`
in `fuzz/`, and the `live/` vectors in `tests/vectors/`. Tests and examples sign with the secret key
of RFC 8032 section 7.1 TEST 1, which is published.
