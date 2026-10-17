# Sluice

Peer-to-peer file transfer that saturates the link. No cloud, no accounts, no third-party service.
Direct device to device over QUIC. Any rendezvous or relay is one you run.

License: Apache-2.0 + MIT dual. Free forever.

Plan v5. Snapshot of the previous pass in `plan-v4.md`.

---

## 0. What the v5 dependency audit changed

The audit asked of every library: maintained, trusted, fast, and survivable if the maintainer stops.
One answer changed the plan.

| Finding | Impact |
|---|---|
| **iroh no longer runs on quinn.** Since v0.96 (Jan 2026) its transport is `noq`, n0's own QUIC, a hard fork of quinn with QUIC Multipath, QUIC NAT Traversal (QNT), and QUIC Address Discovery built in | Every quinn performance citation in v4 was wrong for our stack. `noq` has no independent published benchmarks. Section 2 now treats it as the single largest technical unknown, and M0 measures it before anything depends on it |
| **Good news in the same change.** QNT is an emerging IETF standard, so hole punching now rides inside QUIC with its encryption, loss recovery, congestion control, and DDoS protection. Multipath is already shipped | Multipath moves from "wait for the draft" to "already there". Removed from the deferred list |
| **`iroh-blobs` is not 1.0.** The team has said there may be one more breaking change to the blobs protocol requiring an ALPN change before its 1.0 | Version pinning, our own ticket envelope, and an interop matrix are now hard requirements rather than good practice |
| **n0 is a founder-backed startup, and the whole reuse stack is theirs** (iroh, noq, iroh-blobs, bao-tree, iroh-relay, range-collections) | Concentration risk is named, with a fork and vendor plan per dependency. Section 2.2 |
| `turmoil` 0.7 now simulates filesystem durability and torn writes | Directly tests the fsync barrier model. Better than expected |
| `spake2` has never had an independent audit, by its own README | Short codes stay, with the caveat documented, rendezvous rate limiting, and a note that this is the one place we rely on unaudited crypto |
| `interprocess` dropped | tokio already has Unix sockets and Windows named pipes. One less dependency |
| `fuse3` preferred over `fuser` | Async API, active releases, larger install base |
| iroh-blobs already compiles to WASM | The browser core is less work than v4 assumed |
| **New section 9: can one C# expert build and maintain this** | Skills map, AI-assisted workflow rules, effort split, and escape hatches, since that question determines whether any of this ships |

---

## 1. Position

| Project | Stack | License | Missing |
|---|---|---|---|
| **Blip** | closed, DERP-style relay | paid | Source, self-hosting, API |
| **DashBeam** | Rust, iroh | AGPL-3.0 | Live streaming, range reads, mount, delta, API, SDKs, benchmarks |
| **sendme** | Rust, iroh-blobs | Apache/MIT | Everything but the CLI |
| **croc**, **magic-wormhole** | Go, Python, TCP relay | MIT | Hole punching, speed, resume by hash |
| **LocalSend** | Flutter, HTTPS on LAN | MIT | Works on one LAN |
| **Syncthing** | Go, BEP | MPL-2.0 | One-shot send, streaming reads |
| **Aspera FASP** | closed C, UDP | six figures | Everything open |

DashBeam is the competitor, not Blip. It already has multi-peer, roaming, paired devices, self-hosted
relays, browser, Android, CLI.

> **The claim:** the fastest open source P2P transfer engine, embeddable under Apache-2.0, that can read
> verified byte ranges out of a file still being written, and that an agent can drive over MCP.

Dropped from the brief: Telegram, and the phrase "world's first".

---

## 2. Dependencies: audit, verdicts, escape hatches

### 2.1 The audit

Perf evidence column is deliberate. "Popular" is not "fast", and "fast in a blog post" is not "fast here".

| Dependency | Owner | Maturity | Bus factor | Perf evidence | Verdict |
|---|---|---|---|---|---|
| `iroh` 1.0 | n0 (company) | 1.0, wire-stable, 200M+ endpoint connections a month | Company, ~10k stars | 90% direct-connect rate published. Throughput not published | **Use.** The connectivity problem is the hard one and this is the best answer available |
| `noq` (QUIC) | n0 (company) | Shipped in production since iroh 0.96, tested against picoquic | Company, single implementation, hard fork of quinn | **None published.** Inherits quinn's architecture, but multipath and QNT are new code | **Use, but measure first.** Largest unknown in the plan. See 2.3 |
| `iroh-blobs` | n0 | **0.9x, not 1.0.** One possible ALPN break before 1.0 | Company | Used by sendme and DashBeam at "multi-gigabit" per their claims, unverified | **Use, pinned.** Our own ticket envelope isolates us from their ALPN churn |
| `bao-tree` | n0 (rklaehn) | Stable, narrow scope | Effectively one author | BLAKE3 subtree hashing, optimized fork of `blake3` | **Use.** ~3k lines of pure logic with exhaustive tests. Vendorable in a week if abandoned |
| `range-collections` | rklaehn | Stable, tiny | One author | Pure data structure | **Use.** Vendorable in a day |
| `iroh-relay` | n0 | Ships with iroh | Company | Stateless by design | **Use as a container.** No code of ours |
| `rustls` | ISRG / Prossimo funded | Industry standard, audited | Foundation-backed, many maintainers | Best-in-class TLS, ML-KEM in-crate since 0.23.22 | **Use.** Lowest-risk dependency in the tree |
| `tokio` | tokio-rs | Industry standard | Large team | The reference async runtime | **Use** |
| `tonic` + `prost` | tokio-rs / community | Industry standard | Large | Used at scale in service meshes | **Use** |
| `turmoil` 0.7 | tokio-rs | Active (0.7, Nov 2025), now simulates fs durability and torn writes | tokio-rs org | Test-only, no runtime cost | **Use.** Better fit than expected |
| `zstd` (bindings) | community, wraps Facebook zstd | Mature | Active | Reference implementation underneath | **Use** |
| `fastcdc` | nlfiedler | Stable, v2020 algorithm | One author | Deterministic, benchmarked in-crate | **Use.** ~1k lines, vendorable. `chunkrs` is newer and unproven, so not yet |
| `fuse3` | Sherlock-Holo | Active (0.9, Mar 2026), 188k downloads | One author | Async FUSE | **Use**, feature-gated |
| `winfsp` crate | SnowflakePowered | Active (release within days of audit), 41 versions | One author, wraps mature WinFsp | Thin binding over a mature driver | **Use**, feature-gated. If abandoned, bind the C API ourselves |
| `rmcp` | modelcontextprotocol (official) | v3.0.0 for spec 2026-07-28, 97.5% server conformance | Official project | Protocol-bound, not perf-bound | **Use** |
| `spake2` | RustCrypto | Stable, **no independent audit by its own README** | RustCrypto team | Not perf-relevant | **Use with the caveat documented.** Same choice magic-wormhole made. Short codes are optional and rate-limited |
| `ModelContextProtocol` (C#) | Microsoft + MCP project | v2.x, spec 2026-07-28, `Extensions.Tasks` for long calls | Microsoft-backed | Control plane only | **Use** |
| ASP.NET Core + gRPC on Native AOT | Microsoft | gRPC AOT since .NET 8, production-ready in .NET 10 | Microsoft | 40 to 60% smaller images, sub-100 ms starts | **Use.** MVC is not AOT-compatible, and we do not use MVC |
| Tauri 2 | Tauri (foundation-backed) | 2.11.x mid-2026, stable mobile APIs | Large team | Small bundles, low idle memory | **Use** |
| `interprocess` | community | Fine, but unnecessary | one author | n/a | **Dropped.** tokio has Unix sockets and Windows named pipes |
| `csbindgen` | Cysharp | Mature in the Unity and .NET ecosystem | Company | Codegen only | **Optional.** A ten-function C ABI can be hand-written P/Invoke |

### 2.2 Concentration risk, named

Six dependencies come from one founder-backed company: iroh, noq, iroh-blobs, bao-tree,
range-collections, iroh-relay. If n0 changes direction or runs out of runway, that is our whole lower
stack. Mitigations, in order of cost:

1. **Everything is Apache-2.0 or MIT.** A fork is legal and, for the small crates, cheap.
2. **Wire protocol stability is contractual at iroh 1.0.** A v1 endpoint talks to any other v1 endpoint.
   That means a fork can stay compatible instead of splitting the network.
3. **`bao-tree`, `range-collections`, and `fastcdc` are vendorable in days**, being pure logic with good
   tests. These are the ones with a one-author bus factor, and they are also the easiest to absorb.
4. **`iroh-blobs` is the expensive one to absorb.** It is a real store plus protocol. If it were
   abandoned, the honest answer is a maintained fork, not a rewrite. That is a month, not a year, and
   only if it happens.
5. **`iroh` and `noq` are the ones we would not fork.** The hedge is the `Transport` port: our engine
   talks to a three-method trait, so a second implementation can be added for the paths that do not need
   NAT traversal, which is precisely where the highest throughput targets live (LAN, port-mapped, public
   address). We do not build that on day one, and the seam exists so we can.
6. **Standards alignment reduces lock-in over time.** QNT and QUIC Multipath are IETF work, so other
   implementations will eventually interoperate. That is a slow hedge, not a plan.

Not listed as a mitigation: rebuilding the connectivity stack. Section 1 of every previous version of
this plan explains why, and it is still the correct call.

### 2.3 The one measurement that gates everything

`noq` is a young QUIC implementation with no public throughput numbers, and it sits directly on the
critical path of a project whose headline claim is speed. So M0 answers, with numbers:

1. `iroh` plus `iroh-blobs` throughput on 10 GbE, NVMe to NVMe, against an `iperf3` ceiling.
2. Same on a 100 ms, 0.1% loss netem path.
3. Which congestion controller `noq` uses and whether BBR or an equivalent is available. If it ships
   Cubic or NewReno only, the WAN target is at risk and the fix is an upstream contribution.
4. Whether GSO on send and GRO on receive are enabled. This is worth roughly a factor of two.
5. Whether the rustls provider in use prefers `X25519MLKEM768`.

Outcomes: within 20% of ceiling on LAN and the plan proceeds unchanged. Close and we contribute
patches, which is cheaper than a fork and better for everyone. Far and the `Transport` seam earns its
keep with a second implementation for reachable peers.

Publishing these numbers is itself a contribution. Nobody has benchmarked this stack in public.

---

## 3. Build versus reuse

| Concern | Decision |
|---|---|
| QUIC, multipath, NAT traversal, relay fallback, roaming, mDNS, port mapping | reuse: `iroh` and `noq` |
| Verified streaming tree, range algebra | reuse: `bao-tree`, `range-collections` |
| Content-addressed store, partial files, resume bitmap, outboard cache | reuse: `iroh-blobs` |
| Multi-provider download, request splitting | reuse: `iroh-blobs` downloader |
| Static transfer wire protocol | reuse: `iroh-blobs` protocol, 16 KiB chunk groups, verified ranges |
| Relay and rendezvous server | reuse: `iroh-relay` in our container |
| TLS, hybrid post-quantum key exchange | reuse: `rustls` |
| CDC for delta, compression, PAKE, FUSE, WinFsp, MCP, gRPC, simulation | reuse, per section 2.1 |
| **Live transfer of growing content** | **build.** Nobody has it |
| **Delta hint layer over a verified tree** | **build.** Thin |
| **Session, jobs, events, policy, one control API** | **build.** Our product surface |
| **CLI, mount, doctor, SDKs, hub, UI** | **build.** Our product |

The code we own is the code nobody else wrote. Verified streaming and store durability, the two places a
transfer tool corrupts data, come from a library already running in production.

---

## 4. Architecture

```
   agent (local)      agent (remote), browser, script        terminal
        |                          |                            |
   stdio MCP              Connect / gRPC / HTTP MCP              | argv
        |                          |                            |
        |                  +---------------+                    |
        |                  | sluice-hub    | optional, C# AOT    |
        |                  | policy, audit | multi-user, web UI  |
        |                  +-------+-------+                     |
        |                          | same .proto, no translation |
        v                          v                             v
   +---------------------------------------------------------------------+
   |  sluice          Rust, one static binary                            |
   |  core (session, jobs, events) | live | delta | api | mount | ffi     |
   |  -------------------------------------------------------------------|
   |  iroh-blobs   store, verified streaming, downloader                 |
   |  iroh + noq   QUIC, multipath, QNT hole punch, relay, discovery     |
   +---------------------------------------------------------------------+
              |                                    |
       direct QUIC ~90%                     your relay, ~10%
              v                                    v
        peer endpoint                     iroh-relay container
```

| Unit | What | Idle cost |
|---|---|---|
| `sluice` | Rust static binary: CLI, engine, local API, stdio MCP, mount | Nothing when not running. As a service, section 7.3 |
| `sluice-hub` | C# .NET 10 AOT: multi-user policy, audit, web UI, HTTP MCP, remote API | Only installed when a team needs it |
| `libsluice` | C ABI from the same core, for in-process embedders | It is a library |

Four rules that keep this shape honest:

1. **Bytes never cross a language boundary.** Hub, SDKs, and FFI carry control and events only.
2. **The browser is a peer, not an uploader.** No HTTP endpoint anywhere accepts file bytes. A browser
   sending a file runs the WASM core and speaks the wire protocol.
3. **One service definition.** `proto/sluice/v1` is the only API contract. The core serves it on a local
   socket, the hub serves the same service over the network with auth and policy added, every SDK is
   generated from it. Nothing hand-translates anything.
4. **No daemon required.** `sluice send` is a process that exits. A daemon exists only for
   receive-while-idle, paired devices, and mount, with socket activation so it costs zero until used.

---

## 5. Protocol

**Static content** uses the `iroh-blobs` protocol unchanged: BLAKE3 addressing, 16 KiB chunk groups,
verified range requests, hash sequences for folders. Compatibility bonus: `sendme` and DashBeam can
fetch our static tickets.

**Live content**, ours, on ALPN `sluice/1`. A growing file has no stable BLAKE3 root because a bao
tree's shape depends on total length, so:

```
Segment  = 8 MiB, hashed as its own complete BLAKE3 tree
LogEntry = { seq, segment_root, len, prev_entry_hash }
Frame    = { entry, sig(sender_key) }      # append-only signed chain
Close    = { final_root, total_len, sig }  # becomes ordinary static content
```

Each segment verifies independently, the chain verifies by `prev_entry_hash`, and a segment is readable
the moment it lands. The tail is served at partial length with its own tree, so a subscriber is never
more than one segment behind. Segment size is a latency-versus-signature-overhead knob.

**Delta**, ours. `fastcdc` index as a hint layer only. The sender publishes chunk signatures for content
the receiver may already read; the receiver matches locally and requests what it cannot reconstruct.
Verification is always against the real BLAKE3 root, so a bad match costs a refetch, never corruption.
Signatures flow sender to receiver, so the receiver discloses nothing.

**Tickets and versioning.**

```
Ticket = version | kind | endpoint_id | [addrs] | [relay] | [expiry]
  kind = Static { root, size } | Live { stream_id, sender_key }
```

A ticket is secret material, and the docs say so in the first paragraph. Our envelope wraps whatever
`iroh-blobs` does internally, which is what protects users from their pre-1.0 ALPN churn: their break
becomes our version bump, not a broken ticket format. Capability bitset in the `sluice/1` handshake so
unknown features are ignored, not fatal. CI runs interop of current against N-1 and the oldest supported
release, every release.

---

## 6. Connectivity, honestly

You cannot punch between two NATs with zero infrastructure. Neither peer knows the other's mapped port
until something reachable tells them. That role is **rendezvous**, and it is not **relay**.

| Role | Bandwidth | Needed when |
|---|---|---|
| Discovery | none | Always, unless addresses are in the ticket |
| Rendezvous | signalling only | NAT to NAT on WAN |
| Relay | full transfer | About 10% of paths |

**The serverless WAN path is port mapping.** If either router speaks PCP, NAT-PMP, or UPnP-IGD, the
sender requests a public port and writes the address into the ticket. iroh already does this.

| Mode | Infra | Direct rate | Short codes |
|---|---|---|---|
| Same LAN | none, mDNS | ~100% | yes |
| WAN, port mapping works | none | high | no |
| WAN, own rendezvous | one small VPS | ~90% | yes |
| WAN, rendezvous plus relay | VPS with bandwidth | 100% completion | yes |

Defaults honor the premise: ticket addresses on, mDNS on, port mapping on, your own rendezvous and relay
on when configured, **n0 public relays and n0 DNS off**, mainline DHT off. Enabling the n0 defaults takes
a flag and prints a warning.

`sluice doctor` reports NAT class, port mapping result, MTU, UDP egress, chosen path, and relay
reachability in one copy-pasteable block.

---

## 7. Performance, in use and at rest

### 7.1 Levers

Note the change from v4: these are now things to **verify in `noq`** at M0, not things we know we have.

| Lever | Target setting | Status |
|---|---|---|
| Congestion control | BBR or equivalent | **Verify in `noq`.** If Cubic only, contribute upstream |
| Segmentation | GSO send, GRO receive | **Verify.** Worth roughly 2x |
| Pacing | on | Verify |
| MTU | DPLPMTUD on | Verify. A 1200-byte fallback costs 20% on a 1500 path |
| Windows and buffers | explicit. 1 Gbit/s at 100 ms needs 12.5 MB in flight | Ours to set |
| Multipath | on where two paths exist | Already in `noq` |
| Connections | one connection is about one core. Above 5 Gbit/s, shard ranges across N = min(cores / 2, 4) | Ours |
| Disk | io_uring on Linux, IOCP on Windows, sparse plus `fallocate`. No `sendfile`, since QUIC encrypts in userspace | Ours, feature-gated |
| Hashing | BLAKE3 SIMD, 8 to 15 GB/s per core, outboard cached so a repeat send never rehashes | From `bao-tree` |
| Compression | zstd 1 to 3 only when a 64 KB sample beats 0.9 ratio, off above 2 Gbit/s | Ours |

### 7.2 In-use targets

| Scenario | Target |
|---|---|
| 10 GbE LAN, NVMe to NVMe, 100 GB | >= 8 Gbit/s, within 90% of `iperf3` |
| Single QUIC connection, LAN | >= 4 Gbit/s |
| WAN, 100 ms RTT, 0.1% loss | >= 1 Gbit/s |
| WAN, 200 ms RTT, 2% loss | >= 300 Mbit/s |
| Resume after `kill -9` at 60% of 100 GB | < 2 s to first new byte, at most one fsync barrier re-verified |
| Delta resend, 50 GB with 1% changed | < 3% of bytes on the wire |
| Live mode, write to readable | < 1 s at default segment size |
| Mount, sequential read, 1 MiB readahead | >= 500 MB/s on LAN |
| Browser to native, 20 GB | >= 500 Mbit/s, published as the honest browser number |
| Memory during transfer, any size | < 250 MB RSS |

If M0 says the stack cannot reach these, the targets get revised in public with the measurement
attached. A target quietly dropped is worse than a target missed.

### 7.3 At-rest budgets, CI-gated

| Surface | Idle target | How |
|---|---|---|
| `sluice` binary | < 12 MB stripped | LTO, `panic=abort`, mount and wasm feature-gated |
| Cold start to printed ticket | < 30 ms | Native, no runtime, no work before it is needed |
| `sluice serve` idle RSS | < 12 MB | No preallocated pools, store opened lazily |
| `sluice serve` idle wakeups | 0 per second | Fully event-driven. No polling loop anywhere |
| No process at all when idle | optional | systemd socket activation, launchd on demand, Windows delayed start |
| `sluice-hub` idle RSS | < 40 MB | .NET Native AOT, minimal ASP.NET, no reflection, no idle jobs |
| WASM core | < 1.5 MB brotli, lazy | Loaded when a transfer starts. Page load ships UI only |
| SDKs | zero background threads, zero per-chunk allocations, lazy connect | Control plane only |
| MCP local | stdio process, exits with the agent | No listener, no port, no daemon |
| MCP remote | stateless HTTP per spec 2026-07-28 | Progress as tasks, never polling |
| Metrics, tracing | off by default | No exporter running unless enabled |
| Logs | no filenames or ticket material at default level | Leaking paths is a privacy bug in a transfer tool |

CI fails on a 5% throughput regression, a binary-size regression, or an idle-RSS regression.

---

## 8. Code structure, testing, durability

### 8.1 Eight crates

```
crates/
  sluice-core/   session, jobs, events, policy hooks, tickets. Wraps iroh + iroh-blobs.
                 The only crate that knows both. Also the only crate with hard async.
  sluice-live/   segmented append-only log. Sans-IO, no tokio, pure logic.
  sluice-delta/  CDC index and signature negotiation. Sans-IO, pure logic.
  sluice-api/    the one service: tonic over local socket and optional TCP, plus client.
  sluice-cli/    the `sluice` binary. Hosts `sluice mcp` (stdio, rmcp).
  sluice-mount/  fuse3, winfsp, later FSKit. Feature-gated.
  sluice-ffi/    C ABI for embedders. Only other crate with unsafe.
  sluice-wasm/   browser shell. Feature-gated. iroh-blobs already compiles to WASM.
src/
  Sluice.Hub/    C# AOT: policy, audit, web UI, HTTP MCP, remote API. Optional.
  Sluice.Sdk/    C# SDK over the generated client.
proto/sluice/v1/ one contract, source of every SDK.
ui/              one TypeScript app: PWA build plus Tauri build.
bench/ docker/ docs/adr/ SPEC.md
```

The two protocol crates we own are sans-IO on purpose: bytes and timer ticks in, actions out. That makes
them testable as pure functions, fuzzable with a byte slice, reusable in WASM unchanged, and, as section
9 explains, the easiest Rust in the repo to write and to review.

### 8.2 Testing, scoped to what we own

quinn, noq, and iroh test their own transport. Re-simulating QUIC would be theatre.

| Layer | Method |
|---|---|
| `sluice-live`, `sluice-delta` | Sans-IO unit tests, `proptest` on log and range algebra, `cargo-fuzz` on frame parsing |
| Resume and durability | `turmoil` 0.7 with filesystem durability and torn-write injection at every barrier, seed printed on failure |
| Scheduler, multi-provider | `turmoil` with slow, stalled, and malicious providers |
| Real network | `netem` suite in CI: loss, latency, reorder, partition, MTU black hole |
| Interop | current against N-1 and oldest supported, every release |
| Whole system | 100 GB transfer, killed at random points, byte-exact verification |

### 8.3 Durability

Sparse `.part` file plus a sidecar with the verified range bitmap. `fsync` barrier every 64 MB and on
clean shutdown. On restart, ranges up to the last barrier are trusted and anything after is re-verified,
at most 64 MB. Completion is `fsync` then atomic rename, so a half file never appears at the final path.
Disk-full, permission, and quota errors are resumable states, not panics.

### 8.4 Conventions the CI enforces

`#![forbid(unsafe_code)]` outside `sluice-core` and `sluice-ffi`. `thiserror` in libraries, `anyhow` at
binary edges. One error taxonomy of six variants (`Network`, `Verify`, `Storage`, `Policy`, `Cancelled`,
`Bug`) that every SDK maps to. `tracing` spans keyed by transfer id. No function over 60 lines in the
sans-IO crates. `deny(missing_docs)` on public items. `cargo-deny` for licenses and advisories. One ADR
per non-obvious decision. Every crate README answers, in under 200 words, what it owns, what it must
never do, and where its tests live.

---

## 9. Can one C# expert build and maintain this

An honest answer, because it decides whether anything here ships.

### 9.1 What the work actually is

| Part | Language | Difficulty for you | Share of effort at 1.2 |
|---|---|---|---|
| `sluice-live`, `sluice-delta` | Rust, sans-IO | **Low.** No async, no lifetimes puzzles, no `Send`/`Sync`. Structs, enums, matches, and a state machine. This is the Rust that reads like C# | ~15% |
| `sluice-cli`, `sluice-api` | Rust | **Low to medium.** `clap` and `tonic` are declarative. Mostly wiring | ~10% |
| `sluice-core` | Rust, async | **Medium to high.** The one genuinely hard crate: async lifetimes, cancellation, backpressure | ~15% |
| `sluice-mount`, `sluice-ffi`, `sluice-wasm` | Rust, adapters | **Medium.** Narrow, well-documented APIs, all feature-gated and cuttable | ~10% |
| `Sluice.Hub`, `Sluice.Sdk` | C# | **Your home turf** | ~20% |
| `ui/`, SDK for TypeScript | TypeScript | **Medium.** Ordinary web work | ~20% |
| Bench, CI, packaging, docs | shell, YAML, Markdown | Low, tedious, high value | ~10% |

Roughly half the project is Rust, and only about a sixth of it is the hard kind of Rust. That ratio is
the direct result of the reuse decisions in section 3. In v3 of this plan it was far worse.

### 9.2 Design choices that lower the Rust difficulty, deliberately

These are in the plan for maintainability, not for style points:

- **Actor per transfer, owned state, message passing.** No shared `Mutex<State>`. This sidesteps the
  borrow checker's hardest cases, which are exactly the ones that stall people coming from C#.
- **Sans-IO for the logic we own.** Pure functions are the easiest Rust to write, to review, and to
  hand to an AI with a test as the acceptance oracle.
- **No async traits, no generic public APIs, no macro magic.** Concrete types and enum dispatch. Slower
  to type, dramatically easier to read a year later.
- **`unsafe` confined to two crates**, both narrow, both fuzzed.
- **Feature gates on every hard optional part** (io_uring, mount, wasm, ffi). Each one can be turned off
  and shipped later without touching the core.
- **The `.proto` boundary.** Your C# and TypeScript work never blocks on Rust progress, and vice versa.
  You can spend a week in .NET when you need a break from the borrow checker, and the project still moves.

### 9.3 Working with heavy AI codegen

Rust is a better AI-codegen target than C# or TypeScript, for one reason: the compiler and the borrow
checker reject a large class of plausible-looking wrong code before you ever run it. The failure mode
becomes "does not compile", which is cheap, instead of "compiles and corrupts a file at 3am". Set the
repo up to exploit that:

| Practice | Why |
|---|---|
| `SPEC.md` before code, always | The AI writes to a spec, not to a vibe. Also the artifact that makes a second implementation possible |
| Per-crate `AGENTS.md`, plus the 200-word crate README | Context is scoped to one crate, so generation does not sprawl across boundaries |
| One crate per session, crate README as the contract | Keeps the diff reviewable by one person |
| Property tests and fuzz targets as the acceptance oracle | The way to trust generated protocol code without reading every line |
| `turmoil` seeds on every failure | A reproducible seed makes generated concurrency code auditable, which is otherwise the worst category of AI output |
| Files under 400 lines, functions under 60 in sans-IO crates | Both a human and a model review small units better |
| `cargo test` under 60 s, no network in tests | Fast loop, or the AI loop dominates your day |
| CI gates on perf, binary size, idle RSS, licenses | Generated code cannot quietly regress what matters |
| Conventional commits, one ADR per decision | Six months later, the reason survives, for you and for the model |

**What not to delegate.** Three areas get hand-written or line-by-line reviewed: the fsync barrier and
resume logic, the verified-range trust boundary, and anything inside `sluice-ffi`. Those are where a
subtle error is silent, and silent errors in a file transfer tool end the project's reputation.

### 9.4 Maintenance cost, realistically

- **Dependency churn is the main tax**, not your own code. `iroh-blobs` is pre-1.0 and will break at
  least once. Budget roughly one week per quarter for upgrades. Pin exact versions, run Renovate or
  Dependabot, and write an upgrade note per bump.
- **The interop matrix is what keeps upgrades safe.** Without it, an upgrade silently breaks users on
  older releases and you find out from an issue.
- **Two toolchains is a real cost**, paid at CI setup and at release. It buys you a control plane in the
  language you are fastest in, and independent CI jobs so neither side blocks the other.
- **Scope discipline is the whole game.** The deferred list in section 10 is the artifact that protects
  a solo maintainer. Most one-person projects die of feature count, not of difficulty.

### 9.5 Escape hatches, in order

1. Feature-gate off io_uring, mount, ffi, wasm. All optional, all cuttable, none load-bearing.
2. If `sluice-core` async work stalls, thin it and lean harder on `iroh-blobs` higher-level APIs. Less
   control, still shippable.
3. If Rust stalls entirely, the `.proto` boundary means the CLI plus hub plus SDKs plus UI still work
   against a core that only does what `iroh-blobs` already gives. That is a smaller product, not a dead one.
4. Not an escape hatch: writing the core in C#. .NET has QUIC via msquic but no NAT traversal, no relay
   mesh, no verified-streaming store. Choosing C# for the core means reimplementing iroh and
   iroh-blobs, which is the thing section 3 exists to avoid. The unofficial iroh C# binding does not
   change that, because it would put file bytes across an FFI boundary and it has a bus factor of one.

---

## 10. Releases

**M0, weeks 1 to 2. Measure first.**
The five measurements in 2.3, published. Eight crate skeletons with READMEs and `AGENTS.md`. CI: clippy,
fmt, test, `cargo-deny`, binary-size gate. ADRs for sections 2 and 3.
*Gate: LAN throughput within 20% of `iperf3` and the plan proceeds. Otherwise decide upstream patch or a
second transport, with the measurement attached.*

**1.0, weeks 3 to 12. Engine and CLI.**
`sluice-core` on iroh plus iroh-blobs. Live segmented streaming. Verified range requests. CLI: send,
recv, folders, stdin, tickets, short codes, QR, paired devices, bandwidth caps, receipts, `doctor`,
`--json`. Multi-peer fan-out, multi-provider fan-in with scoring. Relay and rendezvous container with
auth modes. Signed packages for brew, winget, scoop, apt, AUR, nix, docker, plus a verifying installer.
Published benchmark page and connectivity matrix.
*Exit: section 7.2 LAN and WAN targets, resume after `kill -9`, 100 GB between two NATs using only a
self-hosted rendezvous, and every at-rest budget in 7.3 gated in CI.*

**1.1, weeks 13 to 19. Programmable.**
`sluice-api` and the `.proto`. `sluice mcp` stdio server. C# and TypeScript SDKs. `libsluice`. `mount` on
Linux and Windows. Delta update.
*Exit: an agent moves a 40 GB dataset over MCP with no shell and no .NET installed. Delta resend of
50 GB with 1% changed moves under 3% of bytes.*

**1.2, weeks 20 to 28. Reach.**
WASM core and browser PWA. Tauri 2 desktop app on the same TypeScript UI. `Sluice.Hub`. Python SDK.
Helm chart.
*Exit: browser to native transfer of 20 GB on all three engines, with the per-browser ceiling shown in
the UI. Hub adds no measurable throughput cost, because it never sees bytes.*

**Later, ordered by evidence.**
Parking for the both-sides-offline case. macOS mount via FSKit. RaptorQ FEC gated on measured loss.
A second transport if 2.3 says so. Mobile via Tauri 2 with honest iOS background limits. Go, Java, Swift
SDKs, ideally contributed. ML-DSA signatures when the ecosystem moves.

If the timeline slips, cut mount before the browser. The browser is how people try the tool.

---

## 11. Security

- Ed25519 endpoint identity. Dialing a key cannot be redirected to another peer.
- QUIC with TLS 1.3 and hybrid post-quantum key exchange (`X25519MLKEM768`) where the provider supports
  it, confirmed at M0. Confidentiality is post-quantum, identity is not, and the threat model says so.
- A ticket is a capability scoped to one root or stream, with optional TTL and one-time use. Short codes
  use `spake2`, which has no independent audit, so they are optional, rate-limited at the rendezvous,
  and called out in the threat model. This is the only unaudited crypto in the tree.
- Per-range BLAKE3 verification, so neither a relay nor a hostile provider can inject bytes. Providers
  serving failing ranges are scored down and dropped.
- Relay authorization: `allowlist` by default, `token` for a team, `open` only behind an explicit flag
  and a rate limit. Per-key bandwidth and connection quotas in every mode. Operator guidance ships in
  the container: it forwards ciphertext and cannot inspect content.
- Hub binds loopback by default, token auth on every network surface.
- Receive sandbox rejects absolute paths, `..`, symlinks, NTFS alternate streams, and Windows reserved
  names from a manifest, with a fuzz target, because path traversal is the obvious first CVE class here.
- Supply chain: signed artifacts, SBOM, provenance, published checksums, and an installer that verifies
  a signature before executing. `cargo-deny` blocks advisories and unclear licenses in CI.
- `SECURITY.md`, published threat model, disclosure address, from day one.

---

## 12. Adoption and contribution

**Adoption.** One signed binary and a verifying installer, so friction matches croc while speed does
not. A benchmark page with method, hardware, netem profiles, and raw data, which nobody in section 1
has, including the stack we build on. The connectivity matrix published up front, because every rival is
vague about when a server is needed. The Aspera comparison named explicitly. MCP at 1.1, since agent
tooling is where developer tools get discovered in 2026. Apache-2.0 as growth strategy, because
DashBeam is AGPL and no company can embed it, while backup tools, CI systems, and render farms can embed
this and then need their fixes upstream. `SPEC.md` written so a third party can reimplement it. A README
comparison that says where rivals win, Syncthing for sync and LocalSend for zero-config LAN on mobile.

**Contribution.** `cargo test` green from a clean clone with no network, no daemon, no .NET. Good first
issues at about a quarter of open issues. Weekend-sized paths: an SDK, a packaging target, a netem
profile, a mount platform, a `doctor` check, a translation. One ADR per decision. `CONTRIBUTING.md` with
a ten minute path to a merged typo fix. Conventional commits, generated changelog. A public roadmap
carrying the deferred list. Fast review, changelog credit, CODEOWNERS per crate. Governance stated up
front: maintainer-led now, foundation home after 1.0, license irrevocable.

---

## 13. Risks

| Risk | Handling |
|---|---|
| **`noq` throughput is unknown** | M0 measures it before anything depends on it. Upstream patches, then the `Transport` seam |
| `iroh-blobs` pre-1.0 ALPN break | Pinned versions, our own ticket envelope, interop matrix in CI |
| Six dependencies from one startup | Section 2.2: permissive licenses, wire stability at v1, small crates vendorable in days, a fork plan for the one expensive crate |
| Users expect zero infrastructure on WAN | Section 6 matrix up front, port mapping on by default, a five dollar VPS as the honest answer |
| The hard Rust crate stalls a solo maintainer | Section 9.5 escape hatches, and 9.2 design choices that keep the hard part small |
| Resume corruption ships | The one bug that would end trust. Barrier model, `turmoil` torn-write injection, fuzz target on the verified reader, hand-reviewed |
| `spake2` is unaudited | Short codes optional, rate-limited, documented. Not on the default path |
| Feature creep re-inflates the plan | Section 3 says what we do not build, section 10 says what waits |
| Browser cannot write huge files outside Chromium | OPFS path plus the ceiling shown in the UI |
| Open relay abused | Allowlist default, quotas in every mode, operator guidance shipped |
| One-maintainer bus factor | ADRs, `SPEC.md`, per-crate READMEs, CODEOWNERS from M0 |
| Name collision | `sluice` has three small unrelated repos and no file-transfer product. Clear the domain before 1.0 |

---

## 14. Locked decisions

1. No third-party services. n0 public relays and DNS discovery off by default, with a warning.
2. Reuse `iroh`, `noq`, `iroh-blobs`, `bao-tree`, `iroh-relay`. Build live streaming, delta hints,
   session and API, and the clients. Nothing else.
3. `noq` performance is a measurement, not an assumption. M0 gates the plan on it.
4. Eight Rust crates, one optional C# hub, one TypeScript UI. No Go, no Dart.
5. One `.proto` is the only API contract, served as gRPC and Connect. No hand-written REST.
6. Bytes never cross a language boundary. The browser is a peer, never an uploader.
7. No daemon required for one-shot transfers. Socket activation when a daemon is wanted.
8. Live content is a signed segmented append-only log. Delta is a hint layer that cannot affect
   correctness.
9. Sans-IO for the two protocol crates we own, with property tests and fuzzing.
10. Durability is an explicit 64 MB fsync barrier, and the resume target is stated in terms of it.
11. At-rest budgets in 7.3 are CI gates, equal in standing to throughput targets.
12. Actor per transfer, no shared mutable state, no async traits, no generic public APIs, no macros.
    Chosen for maintainability by one person with AI assistance.
13. Apache-2.0 plus MIT everywhere.
14. 1.0 is engine, CLI, live, ranges. 1.1 is API, MCP, SDKs, mount, delta. 1.2 is browser, desktop, hub.

## 15. Open questions

1. Is `sluice.dev` free? Decide before M0 ends. Alternates: Penstock, Chute, Slipway.
2. **What does `noq` actually do?** Congestion controller, GSO and GRO, pacing, MTU discovery,
   achievable throughput. The five measurements in 2.3. Highest-value two days in the plan.
3. Can `iroh-relay` run signalling-only, without transit bandwidth? Turns the five dollar VPS from nice
   into compelling.
4. Does the rustls provider in the iroh dependency tree prefer `X25519MLKEM768` by default?
5. Live segment size: 8 MiB is a guess. Measure the latency-versus-signature-overhead curve.
6. WASM BLAKE3 throughput in Safari and Firefox, which sets the honest browser number in 7.2.
7. GitHub org name, and which foundation to approach after 1.0.

---

## Sources

Reuse targets and their status:
[iroh](https://github.com/n0-computer/iroh),
[iroh 1.0](https://www.iroh.computer/blog/v1),
[iroh 0.96, multipath and QNT](https://www.iroh.computer/blog/iroh-0-96-0-the-quic-multipaths-to-1-0),
[noq announcement](https://www.iroh.computer/blog/noq-announcement),
[iroh-blobs](https://github.com/n0-computer/iroh-blobs),
[iroh-blobs 0.90 upgrade guide](https://www.iroh.computer/blog/iroh-blobs-0-90-changes),
[bao-tree](https://github.com/n0-computer/bao-tree),
[quinn](https://github.com/quinn-rs/quinn),
[rustls PQ](https://www.memorysafety.org/blog/pq-key-exchange/),
[turmoil releases](https://github.com/tokio-rs/turmoil/releases),
[fuse3](https://crates.io/crates/fuse3),
[winfsp crate](https://crates.io/crates/winfsp),
[spake2, RustCrypto](https://github.com/RustCrypto/PAKEs/tree/master/spake2),
[fastcdc-rs](https://github.com/nlfiedler/fastcdc-rs),
[rmcp, official Rust MCP SDK](https://github.com/modelcontextprotocol/rust-sdk),
[MCP C# SDK v2](https://devblogs.microsoft.com/dotnet/announcing-v20-of-the-official-mcp-csharp-sdk/),
[gRPC and Native AOT](https://learn.microsoft.com/en-us/aspnet/core/grpc/native-aot?view=aspnetcore-10.0),
[Tauri 2](https://v2.tauri.app/blog/tauri-20/)

Landscape and measurements:
[DashBeam](https://github.com/tonyantony300/dashbeam),
[Blip on HN](https://news.ycombinator.com/item?id=44648772),
[LocalSend internals](https://deepwiki.com/localsend/localsend),
[Aspera benchmarks](https://asperasoft.com/resources/benchmarks/),
[QUIC benchmark thesis, ETH](https://nsg.ee.ethz.ch/files/public/theses/2024-quic_benchmark_suite/thesis-1.pdf),
[File System Access support](https://www.testmuai.com/learning-hub/file-system-access-api-browser-support/),
[deterministic simulation testing](https://s2.dev/blog/dst)
