# sluice-core

**Owns:** the session layer. Jobs, the transfer queue, events, tickets, directory manifests, small-file
packing, the store lock, identity, and the parallel data path. It wraps `iroh` and `iroh-blobs`, and is
the only crate that talks to both.

**Must never:**

- return a byte to a caller before it is verified against the content root
- open a store whose lock another live process holds
- copy user data into the store when reading in place will do
- leave an `iroh-blobs` access mode at its default. Push stays off, and `get`, `get_many` and `observe`
  are set together, because restricting `get` alone leaves the other two open.

**Notes:** this is the only crate with real async, and one of the few where `unsafe` is allowed. Keep
one actor per transfer, with owned state and messages, and no shared `Mutex`.

**Tests:** `turmoil` for crashes and torn writes at each fsync barrier, the `netem` suite in `bench/`,
and whole-system runs with a 100 GB file and a 100k-file tree.
