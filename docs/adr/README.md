# Architecture decision records

One file per decision that is not obvious from the code. Each has a status, a date and an approver.
An ADR is edited freely while it is Proposed. Once Accepted it is frozen, and a change of mind is a new
ADR that supersedes it, with both linking to each other.

Format: Context, Decision, Consequences. Context gives the forces with numbers. Decision is short and
testable. Consequences include what it costs and what would make us revisit it.

| ADR | Decision | Status |
|---|---|---|
| [0001](0001-reuse-iroh-stack.md) | Reuse iroh, noq and iroh-blobs for transport and storage | Accepted |
| [0002](0002-language-allocation.md) | Rust for the engine, TypeScript for the UI, nothing else in the product | Accepted |
| [0003](0003-one-binary-one-contract.md) | One binary, one control contract, and file bytes stay in Rust | Accepted |
| [0004](0004-live-segmented-log.md) | Live content is a signed log of fixed-size segments | Accepted |
| [0005](0005-delta-is-a-hint.md) | Delta is a hint layer and never decides correctness | Accepted |
| [0006](0006-reference-in-place.md) | Reference files in place; the store holds trees, not data | Accepted |
| [0007](0007-no-io-uring-in-1-0.md) | No io_uring in 1.0 | Accepted |
| [0008](0008-sans-io-and-simulation.md) | Protocol crates do no IO, and concurrency is tested by simulation | Accepted |
| [0009](0009-svelte-for-one-ui.md) | One Svelte 5 UI for the browser and the desktop | Accepted |
| [0010](0010-measure-noq-first.md) | Measure the transport before building on it | Accepted |
| [0011](0011-browser-is-relay-bound.md) | The browser is always relayed, and relay mode is never chosen silently | Accepted |
| [0012](0012-crypto-provider-is-chosen.md) | The TLS crypto provider is chosen explicitly | Accepted |
| [0013](0013-rendezvous-and-relay-are-two-tiers.md) | Rendezvous and relay ship as two separate images | Accepted |
| [0014](0014-browser-build-is-single-threaded.md) | The browser build hashes on one thread and needs no special headers | Accepted |
| [0015](0015-bbrv3-by-default.md) | BBRv3 is the default congestion controller | Accepted |
| [0016](0016-parallel-connections-and-tcp-bulk-path.md) | Shard large transfers across connections, with a TCP bulk path for direct peers | Accepted |
| [0017](0017-bindings-in-one-repository.md) | Bindings for every language, generated, in this repository | Accepted |
