# Architecture decision records

One file per decision that is not obvious from the code. Each has a status, a date and an approver.
An ADR is edited freely while it is Proposed. Once Accepted it is frozen, and a change of mind is a
new ADR that supersedes it, with both linking to each other.

Format: Context, Decision, Consequences. Context gives the forces with numbers. Decision is short
and testable. Consequences include what it costs and what would make us revisit it.

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
| [0012](0012-crypto-provider-is-chosen.md) | The TLS crypto provider is chosen explicitly | Superseded by 0018 |
| [0013](0013-rendezvous-and-relay-are-two-tiers.md) | Rendezvous and relay ship as two separate images | Accepted |
| [0014](0014-browser-build-is-single-threaded.md) | The browser build hashes on one thread and needs no special headers | Accepted |
| [0015](0015-bbrv3-by-default.md) | BBRv3 is the default congestion controller | Accepted |
| [0016](0016-parallel-connections-and-tcp-bulk-path.md) | Shard large transfers across connections, with a TCP bulk path for direct peers | Accepted |
| [0017](0017-bindings-in-one-repository.md) | Bindings for every language, generated, in this repository | Accepted |
| [0018](0018-post-quantum-is-checked-at-the-handshake.md) | Post-quantum key exchange is checked at the handshake | Accepted |
| [0019](0019-relay-starts-through-a-guard.md) | The relay image starts through a guard that refuses an open config | Accepted, extended by 0020 |
| [0020](0020-guard-closes-more-defaults-and-guards-rendezvous.md) | The guard closes three more open defaults, pins the file it checked, and guards rendezvous | Accepted, extended by 0021 |
| [0021](0021-guard-hands-over-a-sealed-copy.md) | The guard hands iroh-relay a sealed copy, and a relay without TLS needs a flag | Accepted |
| [0022](0022-landing-page-on-cloudflare.md) | The landing page lives in `site/`, on Cloudflare Workers | Accepted |
| [0023](0023-cubic-until-the-max-bw-fix.md) | Cubic is the default congestion controller until the BBRv3 max bandwidth fix ships | Accepted, amends 0015, amended by 0024 |
| [0024](0024-congestion-switch-is-delivered.md) | The congestion switch runs in `send` and `recv`, and the started event names it | Accepted, amends 0023 |
| [0025](0025-receiver-says-when-complete.md) | A receiver says when the transfer is complete | Accepted, corrected by 0026 |
| [0026](0026-completion-code-needs-every-root.md) | The completion code counts only for a connection that asked for every root | Accepted, corrects 0025 |
| [0027](0027-stores-run-their-collector.md) | Every blob store runs its collector, and a tag holds each blob while it is used | Accepted |
| [0029](0029-only-the-live-stores-collect.md) | Only the live stores collect, and every store is closed on the way out | Accepted, corrects 0027 |
