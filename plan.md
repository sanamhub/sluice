# Sluice

Peer-to-peer file transfer that saturates the link. No cloud, no accounts, no third-party service.
Direct device to device over QUIC. Any rendezvous or relay is one you run.

License: Apache-2.0 + MIT dual. Free forever. No telemetry, ever.

---

## How to read this plan

This file is the design. Decisions are recorded in [`docs/adr/`](docs/adr/README.md), the wire
protocol is [`SPEC.md`](SPEC.md), and the evidence behind upstream claims is in `docs/research/`. When
this file and an ADR disagree, the ADR is the record, and this file has a bug.

---

## 1. Position

| Project | Stack | License | Missing |
|---|---|---|---|
| **Blip** | closed, DERP-style relay | paid | Source, self-hosting, API |
| **DashBeam** | Rust, iroh | AGPL-3.0 | Live streaming, range reads, mount, delta, API, SDKs, benchmarks |
| **sendme** | Rust, iroh-blobs | Apache/MIT | Everything but the CLI |
| **croc**, **magic-wormhole** | Go, Python, TCP relay | MIT | Hole punching, speed, resume by hash |
| **LocalSend** 1.18 | Flutter on a Rust core, HTTPS on LAN | Apache-2.0 | Works on one LAN. Has a CLI and checksums |
| **ffl / ffl-mcp** | WebRTC with relay fallback | Apache-2.0 | Verified ranges, live content. Already exposes P2P transfer to agents over MCP |
| **Hypercore / Hyperdrive** | JS, Holepunch | MIT / Apache-2.0 | A ticket transfer tool for plain files. It is a log and drive format, and the prior art for our live mode |
| **Syncthing** | Go, BEP | MPL-2.0 | One-shot send, streaming reads |
| **Aspera FASP** | closed C, UDP | six figures | Everything open |

DashBeam is the competitor, not Blip. It already has multi-peer, roaming, paired devices, resume,
self-hosted relays, a browser client, Android, and a CLI. About 9.9k GitHub stars, per a secondary
source.

> **The claim:** the fastest open source P2P transfer engine, embeddable under Apache-2.0, that can read
> verified byte ranges out of a plain file still being written, and that a script or agent can drive
> through a `--json` CLI, a gRPC API, or MCP.

Dropped from the brief: Telegram, and the phrase "world's first".

**Prior art, named rather than ignored.**

- Hypercore has shipped signed append-only Merkle logs with live tailing and sparse verified reads for
  years. Our live log (section 5.2) has the same shape. The difference is in what it carries and where
  it ends up: Sluice carries plain files as BLAKE3 segments, the log closes into an ordinary static
  root, and it lives inside a ticket transfer tool.
- p2panda does signed logs on iroh.
- `ffl-mcp` already does agent-driven P2P transfer.

None of these weaken the combination. They mean the README says "together", not "first".

---

## 2. Dependencies: audit, verdicts, escape hatches

### 2.1 Runtime dependencies

| Dependency | Owner | Maturity | Bus factor | Perf evidence | Verdict |
|---|---|---|---|---|---|
| `iroh` 1.3 | n0 (company) | 1.x, wire-stable. Negotiates ALPNs in `Router` registration order | Company, ~10k stars | 90% direct-connect published. Throughput not published | **Use.** Connectivity is the hard problem and this is the best answer |
| `noq` 1.3 (QUIC) | n0 | In production since iroh 0.96, ~5.2k commits, fuzz targets, `bench/` and `perf/` in tree | Company, hard fork of quinn | **No throughput published.** Internals read from source: Cubic default, BBRv3 available, GSO and GRO on | **Use, measure first.** Largest remaining unknown is the number, not the design. Section 2.3 |
| `iroh-blobs` | n0 | **0.103.1, not 1.0**, and no announced 1.0. One possible ALPN break before it. 0.94.0 to 0.103.0 accept unauthenticated push and are yanked | Company | "Multi-gigabit" claimed by sendme and DashBeam, unverified | **Use, pinned `>= 0.103.1`.** Our ticket envelope isolates their churn. Access modes set explicitly, section 13 |
| `bao-tree` | n0 (rklaehn) | Stable, narrow | One author | Optimized BLAKE3 subtree hashing | **Use.** Pure logic, vendorable in a week |
| `range-collections` | rklaehn | Stable, tiny | One author | Data structure | **Use.** Vendorable in a day |
| `iroh-relay` | n0 | Ships with iroh | Company | Stateless by design | **Use as a container** |
| `rustls` 0.23.46 | ISRG / Prossimo | Audited, industry standard. Earlier versions leak session secrets through `Debug` | Foundation, many maintainers | ML-KEM in-crate, `prefer-post-quantum` a default feature | **Use, `>= 0.23.46`.** Lowest-risk dependency here |
| `tokio`, `tonic`, `prost` | tokio-rs and community | Industry standard | Large teams | Proven at scale | **Use** |
| `turmoil` 0.7 | tokio-rs | Active. The filesystem simulation is behind the `unstable-fs` feature | tokio-rs org | Test-only | **Use**, with the caveat below the table |
| `zstd` | wraps Facebook zstd | Mature | Active | Reference implementation | **Use** |
| `fastcdc` | nlfiedler | Stable, v2020 | One author | Deterministic, benchmarked | **Use.** ~1k lines, vendorable |
| `fuse3` | Sherlock-Holo | Active, 188k downloads | One author | Async FUSE | **Use**, feature-gated |
| `winfsp` | SnowflakePowered | Active, wraps a mature driver | One author | Thin binding | **Use**, feature-gated |
| `rmcp` 3.5 | MCP project (official) | MCP spec 2026-07-28, 110/110 server and 440/440 official conformance tests, back-compatible to 2025-11-25. Releases about every two weeks | Official | Protocol-bound | **Use, pinned exactly.** Counted in the churn budget in 11.4 |
| `spake2` | RustCrypto | Stable, **no independent audit per its own README** | RustCrypto | n/a | **Use with the caveat documented.** Optional feature only |
| `ModelContextProtocol` (C#) | Microsoft + MCP | v2.x, `Extensions.Tasks` for long calls | Microsoft | Control plane | **Use** |
| ASP.NET Core + gRPC, Native AOT | Microsoft | Production-ready in .NET 10 | Microsoft | 40 to 60% smaller images, sub-100 ms start | **Use.** MVC is not AOT-compatible and we do not use MVC |
| Tauri 2 | Tauri, foundation-backed | 2.12.x, stable mobile APIs, MSRV 1.90, no Windows 7. 3.0 in alpha | Large team | Small bundles, low idle memory | **Use** |
| **`compio` / io_uring** | compio-rs | Active, but a separate thread-per-core runtime. `tokio-uring` is dormant | Small team | Adopted by Apache Iggy | **Deferred.** ADR 0007. Revisit only if a profile shows disk in the path |
| `interprocess` | community | Unnecessary | one author | n/a | **Dropped.** tokio has Unix sockets and Windows named pipes |

**The one caveat in that table.** `turmoil`'s simulated filesystem, which section 8.2 leans on for the
durability tests, is gated behind an `unstable-fs` feature and is newer than the rest of the crate. Treat
it as a strong test tool that may change shape, not as a fixed dependency: the durability tests go
through one thin `Fs` trait in `sluice-core`, so if `unstable-fs` moves under us the fallback is a
hand-written fault-injecting `Fs` implementation, which is a day of work. The network half of `turmoil`
is stable and carries no such caveat.

### 2.2 Concentration risk

Six dependencies come from one founder-backed company: iroh, noq, iroh-blobs, bao-tree,
range-collections, iroh-relay. n0 describes itself as partly venture and partly founder backed, and
outside analyses of the project make the same observation this section does: a team smaller than
Protocol Labs, and a long-term outlook that depends on n0's commercialization working. That is not a
reason to avoid the best connectivity library available. It is a reason to keep the exits cheap.

Mitigations, cheapest first:

1. Everything is Apache-2.0 or MIT, so a fork is legal and, for the small crates, cheap.
2. iroh 1.0 guarantees wire stability, so a fork stays compatible instead of splitting the network.
3. `bao-tree`, `range-collections`, `fastcdc` are vendorable in days. They are also the one-author ones.
4. `iroh-blobs` is the expensive one. If abandoned, the answer is a maintained fork, about a month.
5. `iroh` and `noq` we would not fork. The hedge is better than a port we invent: **iroh has custom
   transports**, so a second transport for paths that need no NAT traversal plugs into iroh's own
   extension point rather than into an abstraction we maintain. That is exactly where the highest
   throughput targets live, and it means the escape hatch in 2.3 costs an implementation, not a fork.
6. QNT and QUIC Multipath are IETF work, so interoperability improves over time. A slow hedge.

### 2.3 The measurement that gates everything

`noq` is young, unbenchmarked in public, and directly under a project whose headline claim is speed. M0
measures two things on hardware:

1. `iroh` plus `iroh-blobs` on 10 GbE, NVMe to NVMe, against an `iperf3` ceiling.
2. Same on a 100 ms, 0.1% loss netem path.

Both runs use each congestion controller `noq` ships, Cubic (the default) and BBRv3, swapped through
`TransportConfig::congestion_controller_factory`, and publish both curves.

GSO and GRO are on by default in `noq-udp`, with runtime probing and a silent fallback to one segment.
That fallback costs roughly a factor of two (an estimate). So M0 logs the probed `max_gso_segments` and
`gro_segments` on the bench host and **fails the run if either is 1**, rather than publishing a number
produced by an unnoticed fallback.

The crypto provider is injectable: `QuicClientConfig` and `QuicServerConfig` are `TryFrom` the rustls
configs, `iroh` exposes `EndpointBuilder::crypto_provider()`, and the negotiated group is readable from
`HandshakeData`. Section 13 uses this. Source reading for all three is in
[`docs/research/2026-09-16-open-questions.md`](docs/research/2026-09-16-open-questions.md).

Within 20% of ceiling on LAN and the plan proceeds. Close and we contribute patches. Far and the
`Transport` seam earns its keep. Publishing these numbers is itself a contribution: a search found no
public throughput numbers for `iroh`, `noq` or `iroh-blobs`, and `iroh-netbench` ships a tool but no
results.

---

## 3. Build versus reuse

| Concern | Decision |
|---|---|
| QUIC, multipath, NAT traversal, relay, roaming, mDNS, port mapping | reuse: `iroh`, `noq` |
| Verified streaming tree, range algebra | reuse: `bao-tree`, `range-collections` |
| Content-addressed store, partial files, resume bitmap, outboard cache | reuse: `iroh-blobs` |
| Multi-provider download, request splitting | reuse: `iroh-blobs` downloader |
| Static transfer wire protocol | reuse: `iroh-blobs` protocol |
| Relay and rendezvous | reuse: `iroh-relay` in our container |
| TLS, hybrid post-quantum key exchange | reuse: `rustls` |
| CDC, compression, PAKE, FUSE, WinFsp, MCP, gRPC, simulation, release, signing, benchmarking | reuse, sections 2.1 and 9 |
| **Live transfer of growing content** | **build.** Hypercore has the pattern for its own log format. Nobody has it for plain files on iroh and BLAKE3, so it is ours to build, and we study Hypercore's fork, truncation, and late-join handling first |
| **Delta hint layer over a verified tree** | **build.** Thin |
| **Small-file packing, directory manifest, transfer queue** | **build.** Thin, and the gaps everyone hits |
| **Session, jobs, events, policy, one control API** | **build.** Product surface |
| **CLI, mount, doctor, SDKs, hub, UI** | **build.** Product |

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
   |  core (session, jobs, queue, events) | live | delta | api | mount   |
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
| `sluice-hub` | C# .NET 10 AOT: multi-user policy, audit, web UI, HTTP MCP, remote API | Installed only when a team needs it |
| `libsluice` | C ABI from the same core, for in-process embedders | It is a library |

Four rules that keep the shape honest:

1. **Bytes never cross a language boundary.** Hub, SDKs, and FFI carry control and events only.
2. **The browser is a peer, not an uploader.** No HTTP endpoint anywhere accepts file bytes.
3. **One service definition.** `proto/sluice/v1` is the only API contract, governed by `buf`. The core
   serves it on a local socket, the hub serves the same service over the network with auth and policy
   added, and every SDK is generated from it.
4. **No daemon required.** `sluice send` is a process that exits. A daemon exists only for
   receive-while-idle, paired devices, and mount, with socket activation so it costs zero until used.

---

## 5. Protocol and data model

### 5.1 Static content

`iroh-blobs` protocol unchanged: BLAKE3 addressing, 16 KiB chunk groups, verified range requests, hash
sequences for folders. Compatibility bonus: `sendme` and DashBeam can fetch our static tickets.

### 5.2 Live content, ours, on ALPN `sluice/1`

A growing file has no stable BLAKE3 root, because a bao tree's shape depends on total length.

```
Segment  = 8 MiB, hashed as its own complete BLAKE3 tree
LogEntry = { seq, segment_root, len, prev_entry_hash }
Frame    = { entry, sig(sender_key) }      # append-only signed chain
Close    = { final_root, total_len, sig }  # becomes ordinary static content
```

Each segment verifies independently, the chain verifies by `prev_entry_hash`, and a segment is readable
the moment it lands. The tail is served at partial length with its own tree.

### 5.3 Delta, ours

`fastcdc` index as a hint layer only. The sender publishes chunk signatures for content the receiver may
already read; the receiver matches locally and requests what it cannot reconstruct. Verification is
always against the real BLAKE3 root, so a bad match costs a refetch, never corruption. Signatures flow
sender to receiver, so the receiver discloses nothing.

### 5.4 Directory manifest

Undefined metadata behavior is a bug report on day two, so it is defined here.

```
Entry = { path, kind, size, mtime_utc, mode_bits, [symlink_target] }
  kind = File | Dir | Symlink
```

Rules: relative paths only, normalized to forward slashes and NFC. Empty directories are preserved.
mtime restored by default, `--no-times` to skip. Unix mode bits carried and applied on Unix, ignored on
Windows except the read-only bit. Symlinks are recorded but **not followed**, and are recreated only
with `--links`, never escaping the output root. Hard links are not preserved, and the docs say so.

### 5.5 Small-file packing

100k small files destroys per-file overhead in any chunk-and-hash system, and it is the workload that
makes tools look slow in a review.

Files under a threshold (default 1 MiB) are concatenated into pack blobs of about 64 MiB, with the
manifest recording pack id and offset. Packing is deterministic, ordered by path, so packs dedup across
transfers. Large files stay standalone so range requests and mount still work on them. The receiver
unpacks while streaming, never staging a whole pack in memory.

### 5.6 Tickets and versioning

```
Ticket = version | kind | endpoint_id | [addrs] | [relay] | [expiry]
  kind = Static { root, size } | Live { stream_id, sender_key }
```

A ticket is secret material, and the docs say so in the first paragraph. Our envelope wraps whatever
`iroh-blobs` does internally, so their pre-1.0 ALPN break becomes our version bump, not a broken ticket.
A capability bitset in the `sluice/1` handshake means unknown features are ignored, not fatal.

`SPEC.md` ships **test vectors**: known inputs with expected roots, segment chains, manifests, and pack
layouts, plus a conformance runner. That is what lets a third party implement the protocol and prove it,
and it is how our own refactors stay honest.

### 5.7 Transfer queue

Concurrent transfers need a defined policy, not emergent behavior. Default: 3 active transfers, the rest
queued FIFO. Per-transfer priority (`--priority high|normal|low`). Global and per-peer bandwidth caps
shared fairly across active transfers. Pause, resume, and cancel are first-class states that survive a
daemon restart, since resume is content-addressed anyway.

### 5.8 State on disk, identity, migration

**5.8.1 Import mode.** A content-addressed store implies copying data into the store. That is wrong for
this product: a 100 GB send would double disk usage and halve the speed.

- **Default is reference in place.** Hashing reads the file where it lives. The store keeps only the
  outboard tree and metadata, which is about 0.4% of file size at 16 KiB chunk groups, so 100 GB costs
  roughly 400 MB. Negotiating 64 KiB groups for very large blobs would cut that to about 0.1%, if that
  rule survives section 17 question 6.
- `--copy` imports into the store, for sources that are volatile or on removable media.
- **Receiving writes directly to the destination**, as a sparse `.part` plus an outboard sidecar, then
  `fsync` and rename. Never a store copy followed by a second write to the target.

**5.8.2 The source changed mid-send.** Reference-in-place means the file can move under us. Size, mtime,
and inode (file id on Windows) are recorded at import and re-checked before each range read. A mismatch
fails the transfer with a `Verify` error naming the file. Serving half of one version and half of
another, silently, is the failure mode this prevents. `--rehash-on-change` restarts instead of failing.

**5.8.3 Store layout, quota, garbage collection.**

```
$SLUICE_HOME/            default: XDG data dir, or Known Folders on Windows
  identity.key           Ed25519 secret, 0600
  VERSION                on-disk layout version
  blobs/                 outboards, part files, imported copies
  db/                    tags, jobs, peer trust list
  logs/
```

Default cap of 10 GB for reclaimable store content, LRU eviction of untagged blobs, explicit tags to
pin, and a `sluice gc` command with `--dry-run`. Completed destination files are never store-managed and
never GC candidates. The cap counts outboards, imported copies, and incomplete receives, not files the
user chose to keep.

**5.8.4 Identity.** One Ed25519 secret per user at `$SLUICE_HOME/identity.key`, mode 0600, generated on
first run. `sluice id` prints the public endpoint id. Rotation produces a new endpoint id, which
invalidates every existing pairing and every unexpired ticket that names the old id, so `sluice id
--rotate` says that in a confirmation prompt rather than in a changelog. OS keychain storage is a later
option, not a 1.0 requirement.

**5.8.5 One writer per store.** The blob store is single-writer, and this design has two ways to reach
it. The rule: on startup, take an exclusive lock file holding the pid. If the lock is held by a live
process, the CLI delegates over the control socket instead of opening the store. If the lock is stale,
it is reclaimed. Two engines never touch one store, and the failure to notice this is how a
content-addressed store gets corrupted.

**5.8.6 Migration.** `VERSION` records the on-disk layout. Startup runs forward-only migrations and logs
each one. An older binary meeting a newer layout refuses to start with a clear message rather than
writing to a format it does not understand. `sluice doctor` prints the layout version, and a major bump
prints a backup reminder before running.

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
on when configured, **n0 public relays and n0 DNS off**, mainline DHT off. In iroh terms that is
`RelayMode::Disabled` until the operator names a relay, and `RelayMode::Custom` once they do.
`RelayMode::Default` reaches n0's hardcoded free relays and we never select it silently. Enabling it
takes a flag and prints a warning.

**What that default costs, stated plainly.** With no relay configured: LAN works, port-mapped WAN works,
symmetric-NAT-to-symmetric-NAT fails instead of falling back, and the browser client cannot connect at
all, because a tab has no path except a relay (section 10.1). `sluice doctor` and the first run of the
web UI both say which relay is in use, or say that there is none and what that rules out. Silence here
would produce the single worst support experience the product can have.

**Rendezvous and relay are two images, not one flag.** `iroh-relay`'s `ServerConfig` carries the QUIC
address-discovery server and the WebSocket relay as independent `Option`s, so `relay: None, quic:
Some(..)` is a rendezvous that moves no transfer bytes at all. We ship that as its own container
profile, and the full relay as a second one, because the operator's bill differs by an order of
magnitude between them and a flag hides that. ADR 0013.

Not measured: how much hole punching loses without the relay's side channel. M0 runs the section 6
matrix against a rendezvous-only deployment, and the ~90% in the table above is corrected if the
number disagrees.

**Run your relay closed.** iroh relays support authentication, and managed relays are authenticated by
default, because a relay URL found in a public repo or a screenshot is a free bandwidth
faucet. Our container therefore ships with authorization on and `open` reachable only through an explicit
flag plus a rate limit. Two relays in two regions is the documented production shape, since iroh clients
fail over between them on their own.

**One caveat on that rate limit, because it is not upstream's to give.** `RelayConfig::limits` has a
working per-client byte rate limit, `client_rx`, and two fields that look like connection-accept
limiting, `accept_conn_limit` and `accept_conn_burst`, which upstream documents as **not implemented,
no effect**.

So `open` mode gets byte-rate limiting for free, and connection-rate limiting only if we build it in
front of the relay. Until we do, the docs say byte rate only. A published limit that does nothing is
worse than no limit.

`sluice doctor` reports NAT class, port mapping result, MTU, UDP egress, chosen path, relay reachability,
and relay authorization mode in one copy-pasteable block.

---

## 7. Performance, in use and at rest

### 7.1 Levers

Marked by who owns them. `noq` is not quinn, so nothing here is assumed from quinn's behavior.

| Lever | Target | Owner |
|---|---|---|
| Congestion control | BBR or equivalent | `noq`: Cubic by default, BBRv3 swappable. M0 measures both |
| Segmentation | GSO send, GRO receive | `noq`: on by default, with a silent fallback. M0 fails the run if a probed segment count is 1. Worth roughly 2x, an estimate not yet measured on our hardware |
| Pacing, MTU discovery | on, DPLPMTUD | Verify. A 1200-byte fallback costs 20% on a 1500 path |
| Windows and buffers | explicit; 1 Gbit/s at 100 ms needs 12.5 MB in flight | Ours |
| Multipath | on where two paths exist | Already in `noq` |
| Connections | one connection is about one core. Above 5 Gbit/s shard ranges across N = min(cores / 2, 4) | Ours |
| Disk | positioned reads and writes on a blocking pool, sparse file plus `fallocate`. No io_uring in 1.0, no `sendfile` since QUIC encrypts in userspace | Ours |
| Hashing | BLAKE3 SIMD, 8 to 15 GB/s per core, outboard cached so a repeat send never rehashes | `bao-tree` |
| Compression | zstd 1 to 3 only when a 64 KB sample beats 0.9 ratio, off above 2 Gbit/s | Ours |

### 7.2 In-use targets

| Scenario | Target |
|---|---|
| 10 GbE LAN, NVMe to NVMe, 100 GB | >= 8 Gbit/s, within 90% of `iperf3` |
| Single QUIC connection, LAN | >= 4 Gbit/s |
| WAN, 100 ms RTT, 0.1% loss | >= 1 Gbit/s |
| WAN, 200 ms RTT, 2% loss | >= 300 Mbit/s |
| **100k files averaging 20 KB** | **>= 60% of the throughput of one equivalent large file.** The packing target |
| Resume after `kill -9` at 60% of 100 GB | < 2 s to first new byte, at most one fsync barrier re-verified |
| Delta resend, 50 GB with 1% changed | < 3% of bytes on the wire |
| Live mode, write to readable | < 1 s at default segment size |
| Mount, sequential read, 1 MiB readahead | >= 500 MB/s on LAN |
| Browser to native, 20 GB | >= 500 Mbit/s **through a relay you run**, published with that relay's CPU, NIC, and egress. Browser paths are always relayed, section 10.1 |
| Memory during transfer, any size | < 250 MB RSS |

If M0 says the stack cannot reach these, targets get revised in public with the measurement attached. A
target quietly dropped is worse than a target missed.

### 7.3 At-rest budgets, CI-gated

| Surface | Idle target | How |
|---|---|---|
| `sluice` binary | < 12 MB stripped | LTO, `panic=abort`, mount and wasm feature-gated, `cargo bloat` in CI |
| Cold start to printed ticket | < 30 ms | Native, no runtime, no work before it is needed |
| `sluice serve` idle RSS | < 12 MB | No preallocated pools, store opened lazily |
| `sluice serve` idle wakeups | 0 per second | Fully event-driven. No polling loop anywhere |
| No process at all when idle | optional | systemd socket activation, launchd on demand, Windows delayed start |
| `sluice-hub` idle RSS | < 40 MB | .NET Native AOT, minimal ASP.NET, no reflection, no idle jobs |
| Web UI first load | < 60 KB JS | Svelte 5, ~2 to 3 KB runtime. React would not fit this line |
| WASM core | < 1.5 MB brotli, lazy | Loaded when a transfer starts, not at page load |
| SDKs | zero background threads, zero per-chunk allocations, lazy connect | Control plane only |
| MCP local | stdio process, exits with the agent | No listener, no port, no daemon |
| MCP remote | stateless HTTP per spec 2026-07-28 | Progress as tasks, never polling |
| Metrics, tracing | off by default | No exporter running unless enabled |
| Telemetry | none, ever | Not a setting. There is no code that phones home |

### 7.4 How the gates actually work

Two different mechanisms, because one tool cannot do both:

- **CPU-bound work** (hashing, framing, manifest, packing, range algebra) is measured by **CodSpeed with
  `divan`**, which counts instructions rather than wall clock, so it is stable on shared CI runners and a
  5% gate is meaningful on every pull request.
- **Throughput** cannot be measured that way. It runs on a self-hosted pair of machines, nightly and
  before each release, with `netem` profiles and `iperf3` as the ceiling. Results are published with
  kernel, NIC, CPU, and command lines.
- Binary size, idle RSS, and cold start are checked per pull request, since they are cheap and
  deterministic.
- Test corpora are generated from a seed, so no gigabytes live in git.

---

## 8. Code structure, testing, durability

### 8.1 Eight crates

```
crates/
  sluice-core/   session, jobs, queue, events, policy hooks, tickets, manifest, packing.
                 Wraps iroh + iroh-blobs. The only crate with hard async.
  sluice-live/   segmented append-only log. Sans-IO, pure logic.
  sluice-delta/  CDC index and signature negotiation. Sans-IO, pure logic.
  sluice-api/    the one service: tonic over local socket and optional TCP, plus client.
  sluice-cli/    the `sluice` binary. Hosts `sluice mcp` (stdio, rmcp).
  sluice-mount/  fuse3, winfsp, later FSKit. Feature-gated.
  sluice-ffi/    C ABI for embedders. Only other crate with unsafe.
  sluice-wasm/   browser shell. Feature-gated. iroh-blobs already compiles to WASM.
src/
  Sluice.Hub/    C# AOT: policy, audit, web UI, HTTP MCP, remote API. Optional.
  Sluice.Sdk/    C# SDK over the generated client.
proto/sluice/v1/ one contract, governed by buf.
ui/              one Svelte 5 app: PWA build plus Tauri build.
bench/ docker/ docs/adr/ SPEC.md
```

The two protocol crates we own are sans-IO: bytes and timer ticks in, actions out. Testable as pure
functions, fuzzable with a byte slice, reusable in WASM unchanged, and the easiest Rust in the repo.

### 8.2 Testing, scoped to what we own

quinn, noq, and iroh test their own transport. Re-simulating QUIC would be theatre.

| Layer | Method |
|---|---|
| `sluice-live`, `sluice-delta`, manifest, packing | Sans-IO unit tests, `proptest`, `cargo-fuzz` on frame and manifest parsing |
| Resume and durability | `turmoil` 0.7 with filesystem durability and torn-write injection at every barrier, seed printed on failure |
| Scheduler, queue, multi-provider | `turmoil` with slow, stalled, and malicious providers |
| Real network | `netem` suite: loss, latency, reorder, partition, MTU black hole |
| Protocol conformance | `SPEC.md` test vectors, run in CI against our own implementation |
| Interop | current against N-1 and oldest supported, every release |
| Whole system | 100 GB transfer and a 100k-file tree, killed at random points, byte-exact verification |
| Continuous fuzzing | OSS-Fuzz after 1.0, using the same `cargo-fuzz` targets |

### 8.3 Durability

Sparse `.part` file plus a sidecar with the verified range bitmap. `fsync` barrier every 64 MB and on
clean shutdown. On restart, ranges up to the last barrier are trusted, anything after is re-verified, at
most 64 MB. Completion is `fsync` then atomic rename. Disk-full, permission, and quota errors are
resumable states, not panics.

### 8.4 Conventions the CI enforces

The list lives in one place, [`AGENTS.md`](AGENTS.md) under "Style, enforced by CI", and
`CONTRIBUTING.md` links to it. Two rules that matter to the design: the six-variant error taxonomy is
what every SDK maps to, and every crate README answers, in under 200 words, what it owns, what it must
never do, and where its tests live.

---

## 9. Engineering stack

Everything that is not product code. All of it is a solved problem with a current standard tool.

| Concern | Tool | Note |
|---|---|---|
| Toolchain pinning | `rust-toolchain.toml`, Rust 2024 edition, MSRV in CI matrix | Reproducible for humans and agents. MSRV at least 1.90, which Tauri 2.12 requires |
| Task running | `cargo xtask` | One binary, no extra language, works on Windows |
| Dev environment | devcontainer plus a Nix flake | Either path, both reproducible |
| Test runner | `cargo nextest` | Parallel, per-test process isolation, clear failure output |
| Snapshot tests | `insta` | CLI output and manifest rendering |
| Micro-benchmarks | `divan` | Simpler than criterion, and the CodSpeed default |
| Continuous benchmarking | CodSpeed | Instruction counts, stable on shared runners, per-PR gate |
| CLI benchmarks | `hyperfine` | Cold-start budget |
| Fuzzing | `cargo-fuzz`, then OSS-Fuzz | Free continuous fuzzing for open source |
| Simulation | `turmoil` 0.7 | Filesystem durability and torn writes included |
| Lint and format | `clippy`, `rustfmt`, `taplo`, `biome` for TS | `biome` replaces ESLint plus Prettier |
| Proto governance | `buf lint`, `buf breaking` | Unique request and response message per RPC, enforced |
| Dependency policy | `cargo-deny`, `cargo-udeps`, Renovate | Licenses, advisories, unused deps, updates |
| API compatibility | `cargo-semver-checks` | On `sluice-ffi` and any published crate |
| Versioning and changelog | `release-plz` | Release PRs, version bumps, changelog, crates.io publish |
| Binaries and installers | `dist` 0.33 (formerly cargo-dist) | Shell and PowerShell installers, brew, msi, archives, from one config |
| Signing and provenance | GitHub Artifact Attestations (Sigstore, SLSA), plus OS code signing | Keyless, public transparency log, verify with `gh` or `cosign`. Attestations do not stop SmartScreen or Gatekeeper warnings, and unsigned builds are a recurring complaint about comparable tools. So Windows x86_64 also gets Azure Artifact Signing through `dist` 0.33, and macOS gets notarization |
| SBOM and audit trail | attested SBOM per release, `cargo-auditable` in the binary | The shipped binary carries its own dependency list |
| Container builds | `cargo-chef` layer caching, distroless base, multi-arch | Relay under 30 MB, hub under 60 MB |
| Observability | `tracing`, `tracing-opentelemetry`, OTLP export, optional Prometheus endpoint | Both off by default |
| Async debugging | `tokio-console` | Dev only, feature-gated |
| Config | TOML plus env override, XDG and Known Folders paths | No hidden state in home directory roots |
| CLI ergonomics | `clap` v4 derive, shell completions, `clap_mangen` man pages, `indicatif` progress, `NO_COLOR` respected, `--json` everywhere | Scriptable by default |
| Docs site | Astro Starlight, plus `llms.txt` for agents | The tool is agent-facing, so its docs should be too |
| Repo hygiene | issue forms, `CODE_OF_CONDUCT.md`, `SECURITY.md`, `CODEOWNERS`, SPDX headers, OpenSSF Scorecard badge | Signals maintenance to both humans and package reviewers |

---

## 10. Clients

| Client | Design |
|---|---|
| CLI | In-process engine for one-shot commands, daemon only when one is running. TTY output for humans, `--json` otherwise. **Every transfer shows whether it is direct or relayed, and through which relay**, live in the progress line and as a field in `--json`. This was the most consistent request across community threads, and croc and wormhole are criticized for hiding it |
| Native app | Tauri 2 shell with the core compiled in. No sidecar, no local HTTP, one process |
| Browser | Svelte 5 PWA, WASM core lazily loaded. **Always relayed, see 10.1.** Chromium writes via File System Access API; Firefox and Safari use OPFS with the quota ceiling shown, since neither has a disk picker |
| SDKs | Generated from the one `.proto`. Control plane only, streaming progress, no buffering, no background threads, lazy connect |
| MCP local | `sluice mcp` over stdio in the Rust binary via `rmcp`. No .NET required |
| MCP remote | Hub over HTTP, stateless per spec 2026-07-28, long transfers as tasks |
| REST | Connect protocol on the same handlers as gRPC, so JSON over HTTP works with `curl` and `fetch` |

The UI ships keyboard navigation and WCAG 2.2 AA contrast from the first screen, not retrofitted, and
externalized strings, so a translation is a pull request and not a refactor.

### 10.1 The browser is always relayed

A browser tab cannot open a UDP socket. iroh's WASM build therefore reaches peers over a **WebSocket to
a relay**, and every browser connection is relayed for its whole life. There is no direct path from a
tab, no hole punching from a tab, and iroh carries no WebRTC transport. WebTransport being Baseline in
browsers changes nothing here, because the relay protocol is WebSocket. Traffic stays end to end
encrypted and the relay cannot read it, but it does pass through.

Three consequences, all of which have to be stated in the product rather than discovered:

1. **The browser needs a relay you run.** Locked decision 1 turns n0's public relays off, so an operator
   with no relay has a working CLI and a browser client that cannot connect at all. First run of the web
   UI says which relay it is pointed at, or says plainly that there is none.
2. **Browser throughput is relay throughput, and relay bandwidth is the bill.** The 20 GB browser number
   in 7.2 is a measurement of a VPS, not of a link. It is published as such.
3. **The relay is on the critical path for the browser only.** Native to native stays direct in about
   90% of paths. Cutting the browser never degrades the native product.

This is a limit of our stack, not of browsers. WebRTC data channels do hole-punch from a tab, and
FilePizza and PairDrop use them. One vendor figure puts the TURN fallback at about 20 to 30%; that
figure is a lead, not checked.

iroh ships no WebRTC transport, and its docs list WebRTC, and WebTransport with
`serverCertificateHashes`, only as possible future options. Adding WebRTC ourselves means a second
transport, ICE, and a TURN service beside iroh's, which section 3 rules out. So the browser stays
relayed until iroh ships a browser transport that hole-punches.

ADR 0011 records the decision but does not weigh adding WebRTC ourselves. That reasoning needs a new
ADR, since accepted ADRs are not edited. The README states the limit, because a user finding it on
their own costs more trust than the limit does.

---

## 11. Can one C# expert build and maintain this

### 11.1 Effort split

| Part | Language | Difficulty for you | Share at 1.2 |
|---|---|---|---|
| `sluice-live`, `sluice-delta`, manifest, packing | Rust, sans-IO | **Low.** No async, no lifetime puzzles, no `Send`/`Sync`. Reads like C# | ~15% |
| `sluice-cli`, `sluice-api` | Rust | **Low to medium.** `clap` and `tonic` are declarative | ~10% |
| `sluice-core` | Rust, async | **Medium to high.** The one hard crate: async lifetimes, cancellation, backpressure | ~15% |
| `sluice-mount`, `sluice-ffi`, `sluice-wasm` | Rust adapters | **Medium.** Narrow APIs, feature-gated, cuttable | ~10% |
| `Sluice.Hub`, `Sluice.Sdk` | C# | **Home turf** | ~20% |
| `ui/`, TypeScript SDK | Svelte 5, TS | **Medium.** Ordinary web work | ~20% |
| Bench, CI, packaging, docs | xtask, YAML, Markdown | Low, tedious, high value, and section 9 makes most of it configuration | ~10% |

About half the project is Rust and roughly a sixth of it is the hard kind. These shares are estimates,
not measured. Keeping io_uring out (ADR 0007) removes one more piece of the hard sixth.

### 11.2 Design choices that lower Rust difficulty on purpose

Actor per transfer, owned state, message passing, so the borrow checker's hardest cases never appear.
Sans-IO for the logic we own, because pure functions are the easiest Rust to write, review, and hand to
an AI with a test as the oracle. No async traits, no generic public APIs, no macro magic.

`unsafe` is confined to two narrow crates, both fuzzed. Every hard optional part sits behind a feature
gate. The `.proto` boundary means your C# and TypeScript work never blocks on Rust progress, and you can
switch languages for a week when you need to.

### 11.3 Working with heavy AI codegen

Rust is a better AI-codegen target than C# or TypeScript for one reason: the compiler and borrow checker
reject a large class of plausible-looking wrong code before it runs. The failure mode becomes "does not
compile", which is cheap, instead of "compiles and corrupts a file at 3am".

| Practice | Why |
|---|---|
| `SPEC.md` with test vectors before code | The model writes to a spec, and the vectors are the acceptance oracle |
| Per-crate `AGENTS.md` plus the 200-word README | Context scoped to one crate, so generation does not sprawl |
| One crate per session | Diff stays reviewable by one person |
| Property tests and fuzz targets | How to trust generated protocol code without reading every line |
| `turmoil` seeds on failure | Makes generated concurrency code auditable, otherwise the worst category of AI output |
| Files under 400 lines, functions under 60 in sans-IO crates | Both humans and models review small units better |
| `cargo nextest` under 60 s, no network in tests | Or the loop dominates your day |
| CI gates on perf, size, idle RSS, licenses, proto compatibility | Generated code cannot quietly regress what matters |
| Conventional commits, one ADR per decision | Six months later the reason survives, for you and for the model |

**Not delegated:** the three areas listed in `AGENTS.md` under "Do not delegate these". How outside
contributors use AI is in `CONTRIBUTING.md`.

### 11.4 Maintenance cost

Dependency churn is the main tax, not your own code. `iroh-blobs` is pre-1.0 and will break at least
once: budget about a week per quarter, pin exact versions, run Renovate, write an upgrade note per bump.
The interop matrix and the spec test vectors are what make those upgrades safe.

Two toolchains cost something at CI setup and release, and buy a control plane in your fastest
language. Scope discipline is the whole game, and the deferred list in section 12 is what protects a
solo maintainer.

### 11.5 Escape hatches

1. Feature-gate off mount, ffi, wasm. None are load-bearing.
2. If `sluice-core` async work stalls, thin it and lean harder on `iroh-blobs` higher-level APIs.
3. If Rust stalls entirely, the `.proto` boundary means CLI, hub, SDKs, and UI still work against a core
   that does only what `iroh-blobs` gives. A smaller product, not a dead one.
4. Not an escape hatch: writing the core in C#. .NET has QUIC via msquic but no NAT traversal, no relay
   mesh, no verified-streaming store. That path means reimplementing iroh and iroh-blobs.

---

## 12. Releases

**M0, weeks 1 to 2. Measure first.**
The five measurements in 2.3, published. Eight crate skeletons with READMEs and `AGENTS.md`. CI from
section 9: clippy, fmt, `nextest`, `cargo-deny`, `buf`, CodSpeed, size gate. ADRs for sections 2 and 3.
*Gate: LAN throughput within 20% of `iperf3`, or decide upstream patch versus second transport, with the
measurement attached.*

**1.0, weeks 3 to 12. Engine and CLI.**
`sluice-core` on iroh plus iroh-blobs. Live segmented streaming. Verified range requests. Directory
manifest and small-file packing. Transfer queue with priorities and caps. CLI: send, recv, folders,
stdin, tickets, short codes, QR, paired devices, receipts, `doctor`, completions, man pages, `--json`.
Multi-peer fan-out, multi-provider fan-in with scoring. Relay and rendezvous container with auth modes.
`dist` plus `release-plz` plus attestations for every platform. Benchmark page, connectivity matrix,
`SPEC.md` with test vectors.
*Exit: section 7.2 targets including the 100k-file case, resume after `kill -9`, 100 GB between two NATs
with only a self-hosted rendezvous, every at-rest budget gated in CI.*

**1.1, weeks 13 to 19. Programmable.**
`sluice-api` and the `.proto` under `buf`. `sluice mcp` stdio server. C# and TypeScript SDKs.
`libsluice`. `mount` on Linux and Windows, plus macOS through FSKit if time allows. FSKit works on
macOS 26 through fuse-t 1.2.1 or later and macFUSE 5.1's opt-in backend, and the operations it still
lacks (byte-range locks, flock, ioctl, xattrs) are ones a read-only mount does not need. The Windows
mount requires a WinFsp release with the CVE-2026-7162 fix. Delta update. OSS-Fuzz onboarding.
*Exit: an agent moves a 40 GB dataset over MCP with no shell and no .NET installed. Delta resend of
50 GB with 1% changed moves under 3% of bytes.*

**1.2, weeks 20 to 28. Reach.**
WASM core and Svelte 5 PWA. Tauri 2 desktop app on the same UI. `Sluice.Hub`. Python SDK. Helm chart.
*Exit: browser to native transfer of 20 GB on all three engines, through a self-hosted relay, with the
per-browser ceiling and the relay's specification shown. Hub adds no measurable throughput cost, because
it never sees bytes.*

**Later, ordered by evidence.**
io_uring via `compio` if a profile demands it. Parking for both-sides-offline. macOS mount, if it
missed 1.1.
RaptorQ FEC gated on measured loss. A second transport if 2.3 says so. Mobile via Tauri 2 with honest
iOS background limits. Go, Java, Swift SDKs, ideally contributed. ML-DSA signatures when the ecosystem
moves.

If the timeline slips, cut mount before the browser. The browser is how people try the tool.

---

## 13. Security

- Ed25519 endpoint identity. Dialing a key cannot be redirected to another peer.
- QUIC with TLS 1.3. Hybrid post-quantum key exchange (`X25519MLKEM768`, codepoint `0x11EC`, RFC 10024) is **a provider choice, not a wish**, and the
  precise shape of that choice matters. In rustls 0.23 the `aws-lc-rs` provider offers
  `X25519MLKEM768` in `DEFAULT_KX_GROUPS` **whether or not** `prefer-post-quantum` is set; the feature
  only decides whether it is offered first or last, and it is a default feature. The `ring` provider has
  no ML-KEM at all. **So what removes post-quantum key exchange is selecting `ring`, and that selection
  is a Cargo feature race we do not control:** `noq` picks `aws-lc-rs` only under
  `all(feature = "aws-lc-rs", not(feature = "ring"))`, and `iroh`'s presets prefer `ring` when both are
  enabled. Feature unification means one transitive dependency turning on `ring` silently downgrades the
  whole endpoint to classical key exchange, with no error and no log line. The native build therefore
  passes an `aws-lc-rs` provider explicitly through `EndpointBuilder::crypto_provider()` rather than
  inheriting one, and M0 asserts the negotiated group by downcasting
  `Connection::handshake_data()` to `noq`'s `HandshakeData` and checking
  `negotiated_key_exchange_group == Some(NamedGroup::X25519MLKEM768)`. Both halves are mechanical, and
  upstream ships an example and an equivalent test of its own.
- **The browser build is a second crypto profile.** `aws-lc-rs` carries C and assembly and is not a
  practical WASM target, so `sluice-wasm` uses `ring` and gets classical X25519. Two profiles, and the UI
  says which one a session negotiated rather than implying one number for both. Confidentiality is
  post-quantum on native paths only, identity is Ed25519 and post-quantum on neither.
- **Provider access is set explicitly, never by default.** `iroh-blobs` 0.94.0 through 0.103.0 check only
  the `get` mode and accept push from any peer that can connect, so a ticket holder can fill the
  sender's disk. 0.103.1 rejects push by default and gives `get_many` and `observe` their own modes. We
  set push off, and set `get`, `get_many` and `observe` together to the same per-ticket root allowlist.
  A restriction applied to `get` alone leaves the other two open. A test asserts that a push and an
  off-allowlist `get_many` are both refused. The wire rule lands in `SPEC.md` with test vectors.
- **Endpoint ids are not secret.** Relays and lookup services see them. Ticket secrecy rests on the
  content root and the provider allowlist, never on keeping `endpoint_id` private, and the threat
  model says so.
- **No TLS state in logs.** `rustls` before 0.23.46 leaks master secrets and resumption PSKs through
  `Debug` formatting. No `tracing` field at any level formats a connection, session, or handshake
  object. No lint catches this reliably, so review enforces it, and the pin rules out the leaking versions.
- A ticket is a capability scoped to one root or stream, with optional TTL and one-time use. Short codes
  use `spake2`, which has no independent audit, so they are optional, rate-limited at the rendezvous, and
  named in the threat model. The only unaudited crypto in the tree.
- Per-range BLAKE3 verification, so neither a relay nor a hostile provider can inject bytes. Providers
  serving failing ranges are scored down and dropped.
- Relay authorization: `allowlist` by default, `token` for a team, `open` only behind an explicit flag
  and a per-client byte rate limit. Per-key bandwidth quotas in every mode. Connection-rate quotas are
  **ours to build**, not upstream's to provide: `iroh-relay`'s `accept_conn_limit` and
  `accept_conn_burst` are documented as not implemented. Section 6.
- Hub binds loopback by default, token auth on every network surface.
- **MCP surfaces treat peer data as hostile.** Tool poisoning and prompt injection through tool output
  are the live MCP threats, and coding agents run MCP servers with the developer's privileges.
  Three rules for `sluice mcp` and the hub:
  1. Tool descriptions are static strings compiled into the binary.
  2. Peer-supplied text (paths, symlink targets, relay URLs, error reasons, manifest names) is returned
     only in structured fields marked untrusted, never interpolated into prose.
  3. Any tool that accepts content from or exposes content to a peer needs explicit confirmation or a
     policy allowlist. That covers receiving outside a configured root, pairing, widening relay mode,
     and rotating identity.
- Receive sandbox rejects absolute paths, `..`, symlinks escaping the root, NTFS alternate streams, and
  Windows reserved names from a manifest, with a fuzz target. Path traversal is the obvious first CVE
  class here.
- Supply chain: attested builds with SLSA provenance, attested SBOM, `cargo-auditable` metadata in the
  binary, `cargo-deny` blocking advisories and unclear licenses, and installers that verify before
  executing.
- `SECURITY.md`, published threat model, disclosure address, from day one.

---

## 14. Adoption and contribution

**Adoption.** One signed binary and a verifying installer generated by `dist`. A benchmark page with
method, hardware, netem profiles, and raw data, which nobody in section 1 has, including the stack we
build on. The connectivity matrix published up front. The Aspera comparison named explicitly. MCP at
1.1, listed in the MCP registry (in preview) as one discovery channel among several.
Apache-2.0 as growth strategy, because DashBeam is AGPL and no company can embed it. `SPEC.md` with test
vectors, so a third party can reimplement and prove it. A README comparison that says where rivals win.

**Pitch outcomes, not stack.** Comparable launches show three things:

- Interchangeable P2P senders get no attention. AltSendme's Show HN scored 2 points.
- "Built in Rust" and "built on iroh" earn no goodwill on their own. iroh 1.0's launch thread mostly
  asked what it was for.
- MCP fatigue is real. An "MCP is dead, long live the CLI" post topped HN, and thin MCP
  wrappers are on Thoughtworks' Hold ring.

So the launch leads with what a user can do that they could not before: send a recording while it
records, fetch one verified range of a huge file, and see why a path is relayed. Scripting is presented
as the `--json` CLI first and the gRPC API second, with MCP as one adapter over the same calls, not
the headline. Benchmarks include WAN, NAT and relay-path numbers, not only LAN. Loopback and LAN-only
vendor numbers are routinely dismissed, and croc's author leaves speed out of comparisons for that
reason.

**Contribution.** `cargo nextest` green from a clean clone with no network, no daemon, no .NET. Good
first issues at about a quarter of open issues. Weekend-sized paths: an SDK, a packaging target, a netem
profile, a mount platform, a `doctor` check, a translation. One ADR per decision. `CONTRIBUTING.md` with
a ten minute path to a merged typo fix. Conventional commits and a generated changelog. A public roadmap
carrying the deferred list. Fast review, changelog credit, CODEOWNERS per crate. Governance stated up
front: maintainer-led now, foundation home after 1.0, license irrevocable.

---

## 15. Risks

| Risk | Handling |
|---|---|
| **`noq` throughput unknown** | M0 measures before anything depends on it. Upstream patches, then the `Transport` seam. The levers exist (Cubic default, BBRv3 available, offload on); what is unknown is the number |
| **Crypto provider silently downgrades to `ring`** | Cargo feature unification can pull `ring` in transitively, and both `noq` and `iroh` prefer it when both providers are enabled, which drops post-quantum key exchange with no error. Provider passed explicitly, negotiated group asserted in a test, and a `cargo tree` check in CI. Section 13 |
| **`SPEC.md` 5.2 may be unimplementable** | `iroh-blobs` treats the 16 KiB block size as a constant, so the 64 KiB negotiation for large blobs has no seam. Settled in M0 before the spec grows test vectors. Section 17 question 6 |
| `iroh-blobs` pre-1.0 ALPN break | Pinned versions, our ticket envelope, interop matrix, spec test vectors |
| Six dependencies from one startup | Section 2.2: permissive licenses, wire stability, small crates vendorable in days, fork plan for the expensive one |
| Users expect zero infrastructure on WAN | Section 6 matrix up front, port mapping on by default, a five dollar VPS as the honest answer |
| The hard Rust crate stalls a solo maintainer | Section 11.5 escape hatches, 11.2 design choices that keep the hard part small |
| Resume corruption ships | Barrier model, `turmoil` torn-write injection, fuzz target, hand-reviewed, spec vectors |
| Small-file workloads look slow in reviews | Packing designed in, with an explicit target in 7.2 |
| `spake2` unaudited | Short codes optional, rate-limited, documented, off the default path |
| Feature creep | Section 3 says what we do not build, section 12 says what waits |
| Browser cannot write huge files outside Chromium | OPFS path plus the ceiling shown in the UI |
| Open relay abused | Allowlist default, quotas in every mode, operator guidance shipped |
| One-maintainer bus factor | ADRs, `SPEC.md` with vectors, per-crate READMEs, CODEOWNERS from M0 |
| Store corruption from two engines on one store | Exclusive lock with pid, stale-lock reclaim, CLI delegates to a running daemon. Section 5.8.5 |
| Reference-in-place source mutated mid-send | Size, mtime, inode re-checked before each read. Fail loudly, never serve mixed content |
| On-disk format change strands users | Versioned layout, forward-only migrations, older binary refuses a newer layout |
| **Browser client is useless without a relay**, and locked decision 1 ships without one | Section 10.1. The web UI names its relay on first run, or says there is none and what that rules out. The relay container is one `docker run` and is in the quickstart, not an appendix |
| Relay egress becomes the operator's real bill, since browser paths never leave it | Per-key bandwidth and connection quotas in every authorization mode, quota visible in `doctor`, and the browser number in 7.2 published with the relay's specification |
| Browser sessions get classical key exchange while native gets post-quantum | Stated in section 13 and surfaced per session in the UI, rather than one marketing claim covering both |
| `turmoil`'s `unstable-fs` changes shape under the durability tests | Durability goes through one `Fs` trait; the fallback is a hand-written fault-injecting implementation, about a day |
| Name collision | `sluice` has three small unrelated repos and no file-transfer product. Clear the domain before 1.0 |
| **Upstream access defaults change under us** | `iroh-blobs` 0.94 to 0.103.0 silently accept push. Access modes are set explicitly, not inherited, and an integration test refuses push and off-allowlist `get_many`. Security posts from n0 go into Renovate's advisory feed. Section 13 |
| **Heavy AI codegen costs trust at launch** | curl ended its bug bounty over AI-slop reports, Ghostty and Godot restricted AI contributions, and DashBeam's AI authorship was called out for its security-sensitive code. Answer: the AI policy in `CONTRIBUTING.md`, human ownership of the three no-delegation areas recorded in CODEOWNERS, and fuzzing, property tests, and `turmoil` seeds visible in CI before any public launch |
| **EU Cyber Resilience Act scope is unresolved** | Reporting duties for actively exploited vulnerabilities (24 h, 72 h, 14 days) apply from 2026-09-11, and the rest from 2027-12-11. Non-commercial open source is out of scope, stewards carry lighter duties, and a paid hub or paid support could change the answer. **A legal question, answered by a qualified person before 1.0, not assumed.** `SECURITY.md` is the base either way |
| Overclaiming novelty | Hypercore, p2panda and `ffl-mcp` are prior art for live logs and MCP transfer. The README says "together", names them, and states the narrow difference. Section 1 |

---

## 16. Locked decisions

1. No third-party services. n0 public relays and DNS discovery off by default, with a warning. No
   telemetry, ever, and no code that could send any.
2. Reuse `iroh`, `noq`, `iroh-blobs`, `bao-tree`, `iroh-relay`. Build live streaming, delta hints,
   manifest and packing, session, queue, API, and the clients. Nothing else.
3. `noq` performance is a measurement, not an assumption. M0 gates the plan on it.
4. No io_uring in 1.0. Revisit with `compio` only if a profile shows disk in the path.
5. Eight Rust crates, one optional C# hub, one Svelte 5 UI. No Go, no Dart, no React.
6. One `.proto` is the only API contract, governed by `buf`, served as gRPC and Connect.
7. Bytes never cross a language boundary. The browser is a peer, never an uploader.
8. No daemon required for one-shot transfers. Socket activation when a daemon is wanted.
9. Live content is a signed segmented append-only log. Delta is a hint layer that cannot affect
   correctness. Small files are packed deterministically.
10. Sans-IO for the protocol crates we own, with property tests, fuzzing, and spec test vectors.
11. Durability is an explicit 64 MB fsync barrier, and the resume target is stated in terms of it.
12. At-rest budgets in 7.3 are CI gates. CPU work is gated by CodSpeed per PR, throughput by a
    self-hosted nightly run.
13. Release engineering is `dist` plus `release-plz` plus GitHub artifact attestations. Nothing hand-rolled.
14. Actor per transfer, no shared mutable state, no async traits, no generic public APIs, no macros.
15. Apache-2.0 plus MIT everywhere.
16. Reference in place is the default import mode. The store holds outboards, not user data. One
    exclusive writer per store, and the CLI delegates to a running daemon rather than opening it twice.
17. 1.0 is engine, CLI, live, ranges, manifest, packing, queue, store and identity management. 1.1 is
    API, MCP, SDKs, mount, delta. 1.2 is browser, desktop, hub.
18. Relay mode is `Disabled` until an operator names one, then `Custom`. `RelayMode::Default` is never
    selected silently. Our relay container ships with authorization on. The browser is always relayed
    and the product says so everywhere it matters.
19. Crypto provider is chosen, not inherited: `aws-lc-rs` with `prefer-post-quantum` on native, `ring`
    on WASM, and the negotiated group is asserted in a test rather than assumed.

## 17. Open questions

1. **Name.** `sluice.dev`, `sluice.app`, `slipway.dev`, `penstock.dev` and `chute.dev` are taken, and so
   are the GitHub orgs `sluice`, `sluice-dev` and `sluicehq`. `getsluice.dev` and the org `sluice-p2p` are
   free. Options:
   - keep the name and prefix the handles, at no cost
   - buy `sluice.dev` from the broker, likely four figures
   - rename after a fresh sweep

   This is the one question that gets harder with delay. Evidence in
   [`docs/research/2026-09-16-open-questions.md`](docs/research/2026-09-16-open-questions.md).
2. **`noq` throughput** on LAN and on a lossy path, section 2.3. Needs hardware, and is the
   highest-value two days in the plan.
3. **Direct-connect rate with a rendezvous-only deployment**, which gives up the relay's side channel.
   Section 6 and ADR 0013.
4. **Live segment size and pack size.** 8 MiB and 64 MiB are guesses. Measure both curves. `iroh-blobs`
   pins a 16 KiB block size, which is the verification granularity, so both must be multiples of it and
   `--range` is quantized to it. The guesses are 512 and 4096 blocks, so keeping them until measured
   costs nothing.
5. **BLAKE3 WASM throughput in Safari and Firefox.** It sets the honest browser number and decides
   whether single-threaded SIMD (ADR 0014) holds.
6. **The 64 KiB chunk group rule in `SPEC.md` section 5, rule 2.**
   - **The problem.** The rule cuts outboard overhead from about 0.4% to 0.1% for blobs above 8 GiB.
     `IROH_BLOCK_SIZE` is a constant in `iroh-blobs`, referenced across fourteen files rather than
     passed in, so the rule has no seam while static content is delegated to `iroh-blobs`. `bao-tree`
     is not the obstacle.
   - **Options:**
     - patch upstream to make the block size a store parameter
     - fork `iroh-blobs`
     - bypass `iroh-blobs` for large blobs
     - drop the rule and accept 0.4%, which is about 800 MB of outboard on a 200 GB blob
   - **Deadline.** Settle it in M0, before the spec grows test vectors for a negotiation that cannot
     happen.
7. **EU Cyber Resilience Act scope.** A legal question for a qualified person before 1.0. Section 15.
8. **Three `SPEC.md` rules, each needing test vectors in the same change:**
   - push off, plus `get`, `get_many` and `observe` modes set together (section 13)
   - the ALPN registration order, which `iroh` 1.3 negotiates in
   - what a receiver does with two validly signed frames for one `seq`
9. Which foundation to approach after 1.0. Post-1.0.

---

## Sources

Reuse targets and status:
[iroh](https://github.com/n0-computer/iroh),
[iroh 1.0](https://www.iroh.computer/blog/v1),
[iroh 0.96, multipath and QNT](https://www.iroh.computer/blog/iroh-0-96-0-the-quic-multipaths-to-1-0),
[noq](https://www.iroh.computer/blog/noq-announcement),
[iroh-blobs](https://github.com/n0-computer/iroh-blobs),
[bao-tree](https://github.com/n0-computer/bao-tree),
[rustls PQ](https://www.memorysafety.org/blog/pq-key-exchange/),
[turmoil](https://github.com/tokio-rs/turmoil/releases),
[fuse3](https://crates.io/crates/fuse3),
[winfsp crate](https://crates.io/crates/winfsp),
[spake2](https://github.com/RustCrypto/PAKEs/tree/master/spake2),
[fastcdc-rs](https://github.com/nlfiedler/fastcdc-rs),
[rmcp](https://github.com/modelcontextprotocol/rust-sdk),
[MCP C# SDK v2](https://devblogs.microsoft.com/dotnet/announcing-v20-of-the-official-mcp-csharp-sdk/),
[gRPC and Native AOT](https://learn.microsoft.com/en-us/aspnet/core/grpc/native-aot?view=aspnetcore-10.0),
[Tauri 2](https://v2.tauri.app/blog/tauri-20/),
[compio](https://github.com/compio-rs/compio),
[tokio-uring status](https://users.rust-lang.org/t/status-of-tokio-uring/114481)

Dependency verification:
[noq repository](https://github.com/n0-computer/noq),
[iroh 0.97, custom transports and noq](https://www.iroh.computer/blog/iroh-0-97-0-custom-transports-and-noq),
[iroh WASM and browser support](https://docs.iroh.computer/deployment/wasm-browser-support),
[iroh and the web](https://www.iroh.computer/blog/iroh-and-the-web),
[relay concepts and RelayMode](https://docs.iroh.computer/concepts/relays),
[use your own relay](https://docs.iroh.computer/add-a-relay),
[authenticated relays](https://www.iroh.computer/blog/authenticated-relays),
[iroh 1.0.2, iroh-relay security fix](https://www.iroh.computer/blog/iroh-1-0-2),
[iroh FAQ, sustainability](https://docs.iroh.computer/about/faq),
[iroh-blobs on crates.io](https://crates.io/crates/iroh-blobs),
[rustls-post-quantum and provider requirements](https://docs.rs/rustls-post-quantum/latest/rustls_post_quantum/),
[rmcp README and conformance](https://github.com/modelcontextprotocol/rust-sdk/blob/main/crates/rmcp/README.md),
[turmoil releases](https://github.com/tokio-rs/turmoil/releases),
[dist releases](https://github.com/axodotdev/cargo-dist/releases),
[fuse3 on crates.io](https://crates.io/crates/fuse3),
[WebKit features for Safari 26.4](https://webkit.org/blog/17862/webkit-features-for-safari-26-4/)

Read at the source, full notes in
[`docs/research/2026-09-16-open-questions.md`](docs/research/2026-09-16-open-questions.md):
[noq congestion controllers](https://github.com/n0-computer/noq/tree/main/noq-proto/src/congestion),
[noq TransportConfig defaults](https://github.com/n0-computer/noq/blob/main/noq-proto/src/config/transport.rs),
[noq-udp GSO and GRO](https://github.com/n0-computer/noq/tree/main/noq-udp/src),
[noq rustls seam](https://github.com/n0-computer/noq/blob/main/noq-proto/src/crypto/rustls.rs),
[noq post-quantum test](https://github.com/n0-computer/noq/blob/main/noq/tests/post_quantum.rs),
[iroh EndpointBuilder::crypto_provider](https://github.com/n0-computer/iroh/blob/main/iroh/src/endpoint.rs),
[iroh pq-only-key-exchange example](https://github.com/n0-computer/iroh/blob/main/iroh/examples/pq-only-key-exchange.rs),
[iroh-relay ServerConfig](https://github.com/n0-computer/iroh/blob/main/iroh-relay/src/server.rs),
[iroh-blobs IROH_BLOCK_SIZE](https://github.com/n0-computer/iroh-blobs/blob/main/src/store/mod.rs),
[rustls DEFAULT_KX_GROUPS](https://github.com/rustls/rustls/blob/v/0.23.45/rustls/src/crypto/aws_lc_rs/mod.rs),
[blake3 cargo features](https://github.com/BLAKE3-team/BLAKE3/blob/master/Cargo.toml),
[COOP and COEP](https://web.dev/articles/coop-coep),
[SharedArrayBuffer](https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Global_Objects/SharedArrayBuffer)

Engineering stack:
[dist](https://axodotdev.github.io/cargo-dist/),
[release-plz](https://release-plz.dev/),
[CodSpeed for Rust](https://github.com/CodSpeedHQ/codspeed-rust),
[divan](https://nikolaivazquez.com/blog/divan/),
[buf](https://buf.build/docs/breaking/),
[GitHub artifact attestations](https://docs.github.com/en/actions/concepts/security/artifact-attestations),
[OSS-Fuzz Rust guide](https://google.github.io/oss-fuzz/getting-started/new-project-guide/rust-lang/)

Competitors, prior art, and community, notes in
[`docs/research/2026-10-17-competitors-and-drift.md`](docs/research/2026-10-17-competitors-and-drift.md):
[iroh-blobs push fix](https://iroh.computer/blog/iroh-blobs-push-fix),
[iroh 1.3.0](https://iroh.computer/blog/iroh-1-3-0),
[RFC 10024](https://datatracker.ietf.org/doc/draft-ietf-tls-ecdhe-mlkem/),
[Hypercore](https://github.com/holepunchto/hypercore),
[Hyperdrive](https://github.com/holepunchto/hyperdrive),
[p2panda](https://docs.rs/p2panda),
[ffl-mcp](https://github.com/nuwainfo/ffl-mcp),
[fuse-t](https://github.com/macos-fuse-t/fuse-t),
[MCP tool poisoning, CSA](https://labs.cloudsecurityalliance.org/research/csa-research-note-mcp-tool-poisoning-auto-execution-20260701/)

Market and measurements:
[DashBeam](https://github.com/tonyantony300/dashbeam),
[Blip on HN](https://news.ycombinator.com/item?id=44648772),
[LocalSend internals](https://deepwiki.com/localsend/localsend),
[Aspera benchmarks](https://asperasoft.com/resources/benchmarks/),
[QUIC benchmark thesis, ETH](https://nsg.ee.ethz.ch/files/public/theses/2024-quic_benchmark_suite/thesis-1.pdf),
[File System Access support](https://www.testmuai.com/learning-hub/file-system-access-api-browser-support/),
[frontend bundle comparison](https://www.pkgpulse.com/guides/solidjs-vs-svelte-5-vs-react-reactivity-2026)
