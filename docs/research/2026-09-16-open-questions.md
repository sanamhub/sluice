# Research pass, 2026-09-16: plan.md section 17

Everything below was read from the source of the dependency at the version named, or from a registry
or DNS query run on 2026-09-16. Where a question needs hardware it says so and stays open.

Versions as of this pass: `iroh` 1.2.0 (2026-09-09), `iroh-relay` 1.2.0, `noq` 1.3.0 (2026-09-09),
`iroh-blobs` 0.103.0 (2026-06-15, unchanged), `bao-tree` 0.16.1, `blake3` 1.8.7, `rustls` 0.23.45
(2026-09-14).

---

## Q1 and Q7. The name

`sluice.dev` is **not free.** Registered 2025-06-10 through Name.com, expiry 2027-06-10, parked on
Sedo (91.195.240.94). Every alternate in the plan is also gone.

| Name | State |
|---|---|
| `sluice.dev` | Taken 2025-06-10, Name.com, parked |
| `slipway.dev` | Taken 2026-04-26, Name.com, parked on the same Sedo IP |
| `penstock.dev` | Taken 2026-06-01, live behind Cloudflare |
| `chute.dev` | Taken 2026-09-14, two days before this pass |
| `sluice.app` | Taken 2025-06-12, Porkbun |
| `sluice.io`, `sluice.sh`, `sluice.net`, `sluice.org` | All resolve |
| `usesluice.com` | Registered, no DNS |
| `getsluice.dev`, `sluicehq.dev` | No registry record. Available |

GitHub organizations, checked the same day: `sluice`, `sluice-dev` and `sluicehq` all return HTTP 200,
so all three are taken. `sluice-p2p` returns 404 and is free.

So questions 1 and 7 are one decision, not two, and the bare name is unobtainable on both `.dev` and
GitHub. Three shapes, in increasing order of cost:

1. **Keep the name, prefix the handles.** `getsluice.dev` plus `sluice-p2p` on GitHub. Cheapest. The
   product keeps the name it was designed around; the URLs are second-rate.
2. **Keep the name, buy the domain.** `sluice.dev` is parked with a broker, so it has a price. Unknown
   until asked, and parked `.dev` names of this quality are usually four figures.
3. **Rename.** Requires a fresh availability sweep, because all four names in section 17 are gone and
   one of them was registered two days ago.

**This is the user's call and stays open.** It is also the only open question that gets harder with
delay: `chute.dev` was free when the plan was written and is not now.

---

## Q2. What `noq` actually does

Four of the five measurements in section 2.3 are now answered from source. Two still need hardware.

### 2.3.1 and 2.3.2, throughput on LAN and on a lossy path, still open, needs hardware

No public numbers, in the repo or elsewhere. `noq` carries a `perf/` tool with a JSON output mode and
CI benchmarks comparing itself against upstream quinn, so the harness for M0 is upstream's and does
not have to be written.

### 2.3.3 Congestion controller, answered

`noq-proto/src/congestion/` contains `bbr3`, `cubic.rs`, `new_reno.rs`, exported as `Bbr3`/`Bbr3Config`,
`Cubic`/`CubicConfig`, `NewReno`/`NewRenoConfig`.

The default is **Cubic**: `TransportConfig::default` sets
`congestion_controller_factory: Arc::new(congestion::CubicConfig::default())`.

BBRv3 is available and swappable at runtime through
`TransportConfig::congestion_controller_factory(Arc<dyn ControllerFactory>)`. It is not a stub: `noq`
1.2.0 replaced its older BBR with BBRv3 (#611, a breaking `Controller` API change) and 1.3.0 re-synced
BBRv3 from quinn (#785).

**Consequence for M0:** the controller is a one-line experiment, not a fork. Measure Cubic and BBRv3 on
the same netem profile and publish both curves. A bulk file-transfer product on a fat lossy path is
exactly the case BBR was designed for, so this may be the cheapest throughput win available.

### 2.3.4 GSO and GRO, answered, both on by default

`TransportConfig::default` sets `enable_segmentation_offload: true`. The platform layer is `noq-udp`,
whose own module docs claim "segmentation offload for bulk send and receive operations, reducing CPU
load":

- **Linux:** `UDP_SEGMENT`, requires kernel 4.18+. Probed by setting the option on a real socket; on
  failure it logs and falls back to `max_gso_segments = 1`.
- **Windows:** USO on send and URO on receive, with `max_gso_segments` evaluated per socket and GSO
  disabled at runtime if a send fails, because some Windows versions advertise GSO and then reject it.
- **Apple:** `apple_fast.rs`, opt-in rather than default.

`noq` 1.3.0 hardened all of this: reuse the existing socket for GRO/GSO probing, disable GSO after a
failed probe, and remove an opportunistic GRO syscall.

**Consequence for M0:** measurement 4 becomes an assertion, not a discovery. Log the probed
`max_gso_segments` and `gro_segments` on the bench host and fail the run if either is 1, because a
silent fallback to 1 is roughly the factor of two the plan budgets for.

### 2.3.5 Injected rustls provider, answered, and the assertion is available too

Yes, at both layers, and neither is feature-gated behind something exotic:

- `noq`: `QuicClientConfig: TryFrom<rustls::ClientConfig>` and
  `QuicServerConfig: TryFrom<rustls::ServerConfig>` in `noq-proto/src/crypto/rustls.rs`.
- `iroh`: `EndpointBuilder::crypto_provider(Arc<rustls::crypto::CryptoProvider>)`, with a matching
  `InvalidCryptoProvider` error variant.

Upstream ships the exact pattern section 13 asks for, in `iroh/examples/pq-only-key-exchange.rs`:

```rust
fn pq_only_provider() -> Arc<rustls::crypto::CryptoProvider> {
    let mut p = aws_lc_rs::default_provider();
    p.kx_groups = vec![aws_lc_rs::kx_group::X25519MLKEM768];
    Arc::new(p)
}

let ep = Endpoint::builder(Empty).crypto_provider(pq.clone()) /* ... */;
```

**The assertion is mechanical.** `noq` exposes `HandshakeData.negotiated_key_exchange_group:
Option<NamedGroup>`, made unconditionally available in 1.3.0 (#633), and ships
`noq/tests/post_quantum.rs` asserting `Some(NamedGroup::X25519MLKEM768)`. `iroh` passes the value
through unchanged, since `Connection::handshake_data()` returns `Box<dyn Any>` straight from the inner
`noq` connection. `iroh` does not re-export the type, so our test downcasts to `noq`'s `HandshakeData`.

So M0 item 5 is no longer a question about whether the seam exists. It is a test to write.

### The provider hazard, stated precisely

Section 13 says `X25519MLKEM768` exists "only with the `prefer-post-quantum` feature". That is wrong in
detail. In rustls 0.23.45:

```rust
pub static DEFAULT_KX_GROUPS: &[&dyn SupportedKxGroup] = &[
    #[cfg(feature = "prefer-post-quantum")]
    kx_group::X25519MLKEM768,
    kx_group::X25519,
    kx_group::SECP256R1,
    kx_group::SECP384R1,
    #[cfg(not(feature = "prefer-post-quantum"))]
    kx_group::X25519MLKEM768,
];
```

The group is offered **either way**. The feature controls *preference order*, first or last, nothing
else. And it is on by default: rustls 0.23.45 has
`default = ["aws_lc_rs", "logging", "prefer-post-quantum", "std", "tls12"]`.

What actually removes ML-KEM is **selecting the `ring` provider**, and selection is a Cargo feature
race we do not control:

- `noq`'s `configured_provider()` is `#[cfg(all(feature = "aws-lc-rs", not(feature = "ring")))]` for
  aws-lc-rs, and `#[cfg(feature = "ring")]` for ring. Ring wins when both are on.
- `iroh`'s endpoint presets document the same rule in their own words: the provider is set to ring or
  aws-lc-rs "depending on which feature is enabled in iroh (preferring ring if both are enabled)".

Cargo unifies features across the whole graph. So one transitive dependency enabling iroh's `ring`
feature silently drops the entire endpoint to classical key exchange, with no error and no log line.
That is the real risk, it is invisible by construction, and it is why passing the provider explicitly
through `crypto_provider()` and asserting the negotiated group are both non-negotiable rather than
belt-and-braces.

---

## Q3. Can `iroh-relay` run signalling-only?

**Structurally yes.** `iroh_relay::server::ServerConfig` is:

```rust
pub struct ServerConfig {
    pub relay: Option<RelayConfig>,   // "Configuration for the Relay server, disabled if None"
    pub quic: Option<QuicConfig>,     // "Configuration for the QUIC server, disabled if None"
    pub metrics_addr: Option<SocketAddr>,
}
```

`QuicConfig` is the QUIC Address Discovery server, bound by convention to
`DEFAULT_RELAY_QUIC_PORT` = 7842. So `relay: None, quic: Some(..)` is a running address-discovery
service that carries no transfer bytes at all. The five dollar VPS in section 6 is real.

**What that configuration gives up, and M0 must confirm the size of it.** With `relay: None` there is
no WebSocket side channel, so a peer can learn its own mapped address but has nowhere to send a
coordination message to the other side. Whether that is enough for hole punching depends on how much
of iroh's path setup rides the relay rather than the direct path, and the honest answer today is that
we have read the config surface, not the disco path. The two cases that clearly still work are
addresses carried in the ticket and port-mapped WAN, which is the same row of the section 6 matrix.
The browser client does not work at all in this mode, which section 10.1 already says.

Recommended shape: **two container profiles, not one flag**, `rendezvous` (`quic` only, no transit,
tiny VPS) and `relay` (both, bandwidth bill). ADR 0013 proposes this.

### A correction to sections 6 and 13 found on the way

`RelayConfig::limits` is:

```rust
pub struct Limits {
    pub client_rx: Option<ClientRateLimit>,   // real: bytes_per_second, max_burst_bytes
    pub accept_conn_limit: Option<f64>,       // "Not currently implemented, setting this has no effect."
    pub accept_conn_burst: Option<usize>,     // "Not currently implemented, setting this has no effect."
}
```

Section 13 promises `open` relay mode "only behind an explicit flag and a rate limit", and section 6
repeats it. The **per-client byte rate limit exists**. The **connection-accept rate limit does not**,
upstream documents both fields as no-ops. Either we implement accept limiting ourselves in front of the
relay, or we stop promising it. A rate limit that silently does nothing is worse than no rate limit,
because it is written down.

---

## Q5. Live segment size and pack size

Still guesses, still need the curves measured. One hard constraint found that the plan did not state:

`iroh-blobs` pins `IROH_BLOCK_SIZE = BlockSize::from_chunk_log(4)`, documented in-source as
"Block size used by iroh, 2^4*1024 = 16KiB". `bao-tree` recommends the same value as "a good default
for most cases".

That is the **verification granularity**, so:

- `sluice recv --range 4G:4.1G` is quantized to 16 KiB boundaries. A request for an arbitrary byte
  range fetches and verifies the enclosing blocks. Worth saying in `SPEC.md`, because it is observable.
- The live segment size and the pack size must both be multiples of 16 KiB. The current guesses are:
  8 MiB = 512 blocks, 64 MiB = 4096 blocks. Both clean, so the guesses cost nothing to keep until M0
  measures them.

### A spec rule that may not survive contact with the dependency

`SPEC.md` section 5 rule 2 says: "Chunk group size MAY be negotiated to 64 KiB for blobs above 8 GiB, to
reduce outboard overhead from about 0.4% to about 0.1% of content size."

`IROH_BLOCK_SIZE` is a **constant**, not a parameter, and it is referenced across fourteen files of
`iroh-blobs` including `get.rs`, `api/remote.rs`, `store/fs.rs`, `store/mem.rs` and `store/fs/bao_file.rs`.
`bao-tree` itself is perfectly happy with other block sizes, so the restriction is `iroh-blobs`, not the
format.

So on a plain reading, that rule is **not implementable while delegating static content to
`iroh-blobs`**, and the plan delegates static content to `iroh-blobs` in section 3. The options are a
patch upstream making the block size a store parameter, a fork, bypassing `iroh-blobs` for large blobs,
or dropping the rule and accepting 0.4% outboard overhead, which on a 200 GB blob is about 800 MB.

**This was not chased to the bottom in this pass** and the conclusion rests on reading call sites rather
than attempting the change. It should be settled in M0, before `SPEC.md` acquires test vectors for a
negotiation that cannot be performed. Flagging rather than fixing, because `SPEC.md` is the wire and
`AGENTS.md` says the wire does not change without test vectors in the same commit.

---

## Q6. WASM BLAKE3 and cross-origin isolation

Two halves. The hosting half is answerable now and is a decision. The throughput half is not.

**Cross-origin isolation is required for WASM threads, and only for threads.** Threads need
`SharedArrayBuffer`, which since Spectre requires the document to be cross-origin isolated:
`Cross-Origin-Opener-Policy: same-origin` and `Cross-Origin-Embedder-Policy: require-corp`, with every
cross-origin subresource opting in through CORP or CORS. This is not a Safari quirk, it holds in
Chrome, Firefox and Safari alike, and mobile Safari is the strictest about which COEP variants it
accepts.

The cost is in hosting, exactly as the open question suspected: a host that cannot set response
headers cannot serve a cross-origin-isolated PWA. GitHub Pages is the obvious casualty.

**`blake3` 1.8.7 separates the two concerns cleanly.** `wasm32_simd` is a SIMD implementation with **no
runtime detection**, the build asserts WASM SIMD rather than probing for it, which is fine, since WASM
SIMD is Chrome 91+, Firefox 89+, Safari 16.4+. Multithreading is a *different* feature, `rayon`, and it
is the one that would drag in `SharedArrayBuffer` and the headers.

**So the recommendation is: single-threaded `wasm32_simd`, no threads, no isolation headers.** The
justification is section 10.1's own finding, every browser transfer is relayed for its whole life, so
the ceiling is the relay's bandwidth, not the tab's hash rate. Paying for cross-origin isolation to
speed up a stage that is not the bottleneck is a bad trade, and it costs the PWA the ability to be
hosted anywhere. ADR 0014 proposes this.

**Still open:** actual BLAKE3 WASM SIMD throughput in Safari and Firefox. No public per-browser figures
exist. This is a 20-line benchmark page, and it must be run before any browser throughput number is
published, because it is the number that decides whether the single-threaded choice above holds.

---

## Summary

| Question | State after this pass |
|---|---|
| 1 + 7. Name and org | **Answered, and it is bad news.** `sluice.dev` and the GitHub org are taken, as are all four alternates. Needs a decision |
| 2.3.1, 2.3.2. Throughput | **Open.** Needs hardware. M0 |
| 2.3.3. Congestion controller | **Answered.** Cubic default, BBRv3 available and swappable in one line |
| 2.3.4. GSO and GRO | **Answered.** On by default, both directions, all three platforms. M0 asserts rather than discovers |
| 2.3.5. Injected provider | **Answered.** Seam exists in both `noq` and `iroh`, upstream example included, negotiated group readable. Now a test to write |
| 3. Signalling-only relay | **Answered structurally.** `relay: None, quic: Some(..)`. Two container profiles. Hole-punch impact needs M0 |
| 5. Segment and pack size | **Open**, but constrained: multiples of 16 KiB, the `iroh-blobs` block size. Separately, `SPEC.md` section 5 rule 2 looks unimplementable against `iroh-blobs` and needs settling |
| 6. WASM BLAKE3 | **Half answered.** Isolation headers are required only for threads, so avoid threads. Per-browser throughput still needs measuring |

## Sources

Read at the versions named above:
[noq congestion](https://github.com/n0-computer/noq/tree/main/noq-proto/src/congestion),
[noq TransportConfig defaults](https://github.com/n0-computer/noq/blob/main/noq-proto/src/config/transport.rs),
[noq-udp platform layer](https://github.com/n0-computer/noq/tree/main/noq-udp/src),
[noq rustls seam](https://github.com/n0-computer/noq/blob/main/noq-proto/src/crypto/rustls.rs),
[noq post_quantum test](https://github.com/n0-computer/noq/blob/main/noq/tests/post_quantum.rs),
[noq CHANGELOG](https://github.com/n0-computer/noq/blob/main/CHANGELOG.md),
[iroh EndpointBuilder::crypto_provider](https://github.com/n0-computer/iroh/blob/main/iroh/src/endpoint.rs),
[iroh pq-only-key-exchange example](https://github.com/n0-computer/iroh/blob/main/iroh/examples/pq-only-key-exchange.rs),
[iroh Connection::handshake_data](https://github.com/n0-computer/iroh/blob/main/iroh/src/endpoint/connection.rs),
[iroh-relay ServerConfig](https://github.com/n0-computer/iroh/blob/main/iroh-relay/src/server.rs),
[iroh-blobs IROH_BLOCK_SIZE](https://github.com/n0-computer/iroh-blobs/blob/main/src/store/mod.rs),
[bao-tree BlockSize](https://docs.rs/bao-tree/latest/bao_tree/),
[rustls 0.23.45 DEFAULT_KX_GROUPS](https://github.com/rustls/rustls/blob/v/0.23.45/rustls/src/crypto/aws_lc_rs/mod.rs),
[blake3 features](https://github.com/BLAKE3-team/BLAKE3/blob/master/Cargo.toml)

Cross-origin isolation:
[COOP and COEP, web.dev](https://web.dev/articles/coop-coep),
[SharedArrayBuffer, MDN](https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Global_Objects/SharedArrayBuffer),
[WASM SIMD browser support](https://www.testmuai.com/learning-hub/wasm-simd-browser-support/)

Registry and DNS queries run 2026-09-16: RDAP through `rdap.org`, DNS through the local resolver,
GitHub organization existence by HTTP status.
