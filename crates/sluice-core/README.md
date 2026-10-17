# sluice-core

**Owns:** the session layer. Jobs, the transfer queue and priorities, events, policy hooks, tickets,
directory manifests, small-file packing, the store lock, and identity. Wraps `iroh` and `iroh-blobs`,
and is the only crate that knows about both.

**Must never:** expose an unverified byte to a caller, open a store that another live process holds the
lock on, or copy user data into the store when reference-in-place will do. Never leave an `iroh-blobs`
access mode at its default: push stays off, and `get`, `get_many` and `observe` are set together, since
restricting `get` alone leaves the other two open (`plan.md` section 13).

**Note:** this is the one crate with hard async, and the one place `unsafe` is permitted besides
`sluice-ffi`. Keep the actor-per-transfer shape: owned state, message passing, no shared `Mutex`.

**Tests:** `turmoil` for crash and torn-write injection at fsync barriers, `netem` suites in `bench/`,
whole-system tests with a 100 GB blob and a 100k-file tree.
