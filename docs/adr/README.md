# Architecture Decision Records

One file per non-obvious decision. Numbered, never renumbered, never deleted. A decision that turns out
wrong gets a new ADR superseding the old one, and the old one stays for the reasoning.

Format: Context, Decision, Consequences, Status. Short. If it needs more than a page, it is probably two
decisions.

| ADR | Decision | Status |
|---|---|---|
| [0001](0001-reuse-iroh-stack.md) | Reuse iroh, noq, iroh-blobs instead of building transport and store | Accepted |
| [0002](0002-language-allocation.md) | Rust data plane, C# control plane, TypeScript UI, no Go | Accepted |
| [0003](0003-two-units-one-contract.md) | Two shipping units, one `.proto`, bytes never cross a language boundary | Accepted |
| [0004](0004-live-segmented-log.md) | Live content is a signed segmented append-only log | Accepted |
| [0005](0005-delta-is-a-hint.md) | Delta is a hint layer that cannot affect correctness | Accepted |
| [0006](0006-reference-in-place.md) | Reference user files in place, the store holds outboards | Accepted |
| [0007](0007-no-io-uring-in-1-0.md) | No io_uring in 1.0 | Accepted |
| [0008](0008-sans-io-and-simulation.md) | Sans-IO protocol crates plus deterministic simulation testing | Accepted |
| [0009](0009-svelte-for-one-ui.md) | Svelte 5 for a single UI shared by browser and desktop | Accepted |
| [0010](0010-measure-noq-first.md) | `noq` performance is gated by measurement before the plan proceeds | Accepted |
| [0011](0011-browser-is-relay-bound.md) | The browser is a relayed peer, and relay mode is never chosen silently | Accepted |
| [0012](0012-crypto-provider-is-chosen.md) | The rustls provider is chosen explicitly, browser is a second crypto profile | Accepted |
| [0013](0013-rendezvous-and-relay-are-two-tiers.md) | Rendezvous and relay ship as two container profiles, not one flag | Proposed |
| [0014](0014-browser-build-is-single-threaded.md) | Browser build is single-threaded WASM SIMD, no cross-origin isolation | Proposed |
