# Sluice design

How Sluice is put together, and why. The wire format is in [`SPEC.md`](../SPEC.md). Targets and the
benchmark method are in [`performance.md`](performance.md), the threat model in
[`threat-model.md`](threat-model.md), and the order of work in [`roadmap.md`](roadmap.md). Each
non-obvious decision has an ADR in [`adr/`](adr/README.md), and this document links to it rather
than repeating the argument.

## Goals

Move files between two machines as fast as the path allows, with no account and no service in the
middle that we or anyone else runs for you.

- Fill the link: on 10 GbE with NVMe at both ends, within 10% of `iperf3`.
- Verify every byte against a BLAKE3 root before anything sees it.
- Resume after any interruption, including `kill -9`, without re-sending verified data.
- Read a file while it is still being written, and read any byte range of any file.
- Work on a LAN with no infrastructure, and on the internet with infrastructure you can run on the
  smallest VPS.

Not goals: folder sync (Syncthing does it well), a hosted service, accounts, telemetry.

## Architecture

```
   terminal          script, app, agent                 browser tab
      |                     |                                |
     argv          SDK (in process) or                  WASM build
      |            control API (local socket)                |
      v                     v                                v
   +-------------------------------------------------+   +-------------+
   |  sluice: one Rust binary, or libsluice          |   | sluice-wasm |
   |  session, jobs, queue | live | delta | mount    |   +------+------+
   |  -----------------------------------------------|          |
   |  iroh-blobs   store, verified streaming         |          |
   |  iroh + noq   QUIC, hole punching, relay        |          |
   +-------------------------------------------------+          |
              |                         |                       |
         direct path                relayed path          always relayed
              v                         v                       v
        peer endpoint            iroh-relay you run      iroh-relay you run
```

Four rules hold everywhere:

1. **File bytes stay in Rust.** SDKs, the control API and MCP pass paths, tickets and events. No
   binding takes or returns a buffer of file content. ADR 0003.
2. **The browser is a peer.** No HTTP endpoint accepts file bytes.
3. **One control contract.** `proto/sluice/v1` is the only API definition, linted and checked for
   breaking changes with `buf`.
4. **No daemon for one-shot transfers.** `sluice send` exits. A daemon runs only for
   receive-while-idle, paired devices and mount, and socket activation keeps it at zero cost until
   used.

## Reuse and build

The connectivity, QUIC and verified-streaming layers come from the iroh project. Writing them again
would put the parts where transfer tools corrupt data into new code. ADR 0001.

| Concern | Source |
|---|---|
| QUIC, multipath, hole punching, relay, mDNS, port mapping | `iroh`, `noq` |
| Verified streaming tree, range sets | `bao-tree`, `range-collections` |
| Content-addressed store, partial files, resume, multi-provider download | `iroh-blobs` |
| Relay server | `iroh-relay`, packaged as one container |
| TLS 1.3, hybrid post-quantum key exchange | `rustls` with `aws-lc-rs` |
| Content-defined chunking, compression, PAKE, FUSE, WinFsp, MCP, gRPC | `fastcdc`, `zstd`, `spake2`, `fuse3`, `winfsp`, `rmcp`, `tonic` |
| Live transfer of a growing file | **ours** |
| Delta hints over a verified tree | **ours** |
| Directory manifest, small-file packing, transfer queue | **ours** |
| Parallel data path, TCP bulk path | **ours** |
| Session layer, control API, CLI, mount, bindings, UI | **ours** |

Live transfer has prior art in Hypercore's signed append-only logs. Ours carries plain files as
BLAKE3 segments and closes into an ordinary static root; ADR 0004 covers the differences.

### Dependencies

| Crate | Version | Notes |
|---|---|---|
| `iroh`, `iroh-relay` | 1.4 | Wire-stable since 1.0. Negotiates ALPNs in `Router` registration order |
| `noq` | 1.4 | n0's fork of quinn. Cubic, BBRv3 and NewReno ship; we select Cubic today (ADR 0023). GSO and GRO on by default, with a silent fallback |
| `iroh-blobs` | 0.103.1 or later | Pre-1.0. Versions 0.94.0 to 0.103.0 accept unauthenticated push and are yanked |
| `bao-tree`, `range-collections` | 0.16, 0.4 | Small and single-author; vendorable in days |
| `rustls` | 0.23.46 or later | Earlier releases leak session secrets through `Debug` |
| `rmcp` | 3.5 | Official MCP SDK. Releases about every two weeks, so pinned exactly |
| `turmoil` | 0.7 | Test only. The filesystem simulation sits behind `unstable-fs` |

Six of these come from one company, n0. The exits stay cheap: everything is Apache-2.0 or MIT,
iroh's wire is stable, the small crates can be vendored in days, and `iroh-blobs` is the one fork
that would take about a month. iroh's custom transport hook is where a second transport plugs in
without forking anything.

## Data model

### Static content

The `iroh-blobs` protocol, unchanged: BLAKE3 addressing, 16 KiB chunk groups, verified range
requests, hash sequences for collections. `sendme` and DashBeam can fetch our static tickets.

The 16 KiB block size is a constant in `iroh-blobs`. Verified ranges are quantized to it, and live
segment sizes must be multiples of it. Pack sizes need not be: a pack holds its files end to end
with no padding, a receiver fetches a pack whole, and its last group is verified like the end of any
blob.

### Live content

A growing file has no stable BLAKE3 root, because a bao tree's shape depends on total length. Live
content is a signed chain of fixed-size segments on ALPN `sluice/1`:

```
Segment  = 8 MiB, hashed as its own complete BLAKE3 tree
LogEntry = { seq, segment_root, len, prev_entry_hash }
Frame    = { entry, sig(sender_key) }
Close    = { final_root, total_len, sig }
```

Each segment verifies on its own and is readable the moment it lands. The unsealed tail is served at
its current length and re-verified when its entry arrives. After `Close`, the file is ordinary
static content. ADR 0004, and `SPEC.md` section 6 for the exact rules.

### Delta

A FastCDC index used only as a hint. The provider sends chunk signatures for content the requester
may already read; the requester matches locally and fetches the rest. The result is verified against
the real root, so a wrong match costs a refetch. The requester discloses nothing about its disk. ADR
0005.

### Directory manifest

```
Entry = { path, kind, size, mtime_utc, mode_bits, [symlink_target] }
  kind = File | Dir | Symlink
```

Paths are relative, NFC-normalized and forward-slashed. Empty directories are kept. mtime is
restored unless `--no-times`. Unix mode bits apply on Unix; Windows honors only the read-only bit.
Symlinks are recorded, never followed, and recreated only with `--links`, never outside the output
root. Hard links are not preserved.

### Small-file packing

Files under 1 MiB are concatenated into pack blobs of at most 64 MiB, in path order, so identical
trees produce identical packs. The sizes are fixed constants, not settings, and empty files are
never packed. Large files stay standalone so ranges and mount work on them. A receiver may fetch a
whole pack and unpack it from its store, or unpack it while streaming; it never holds a whole pack
in memory. Without this, 100k small files spend more time on per-file overhead than on data.

### Tickets

```
Ticket = version | kind | endpoint_id | [addrs] | [relay] | [expiry]
  kind = Static { root, size } | Live { stream_id, sender_key }
```

A ticket is a read capability, so it is secret. Our envelope wraps whatever `iroh-blobs` uses
internally, so an `iroh-blobs` protocol change becomes a version bump in our ticket rather than a
broken one.

### Transfer queue

Three active transfers by default, the rest queued in order. Per-transfer priority (`--priority
high|normal|low`). Global and per-peer bandwidth caps, shared fairly. Pause, resume and cancel
survive a daemon restart, since resume is content-addressed. A size is written as `50M` (1024²) or
`50MB` (1000²); `--limit-rate`, `--max-size` and `--range` share one parser, so a unit means the same
in every flag.

## Storage

**Reference in place.** Sending hashes the file where it lives. The store keeps only the outboard
tree and metadata, about 0.4% of file size, so a 100 GB send costs about 400 MB. `--copy` imports
into the store for volatile or removable sources. ADR 0006.

**A send keeps only what it copies, in a store of its own.** Standard input, the packs of small
files, the manifest and the hash sequence, and the segments of a live stream are copied into a store
in a private folder under the home's temporary area. The folder and its store are removed when the
send ends, however it ends, and a folder a killed send left is removed by the sweep of temporary
folders. So a home that sends keeps nothing of what was sent, and two sends in one home never share
a store. Regular files are still sent where they live. ADR 0030.

**Receiving uses a store beside the destination.** `iroh-blobs` keeps partial data in its own store,
not in a sparse `.part` of ours, so a receive makes a hidden per-transfer store in the output folder,
`.sluice-partial-<ticket root prefix>`. It is on the same filesystem as the destination, which lets
a finished blob be exported by move rather than copied. A 100 GB receive therefore peaks at about
100 GB of payload plus the outboard and metadata, not 200 GB. One store holds a whole folder, so a
resumed folder receive asks only for the ranges it still lacks. The store is removed after every
file is in place; after a failure it is kept for resume. The receiver checks free space for the
whole tree before it fetches content and refuses with `Storage` when the volume cannot hold it.
ADR 0006.

**Only the live stores collect.** A store is opened with `iroh-blobs`' collector only where every
blob in it is throwaway: the live send and the live receive. A live receive holds each segment until
it is written out, and frees it at the next sweep, ten seconds later, so a stream keeps about that
much beside its output instead of a second copy of everything it has written. A live send holds every
segment until it stops, because a reader may ask for any of them until then, and what it leaves is
bounded by its folder going away. Every other store keeps its bytes until the command is done with
them: a resume store has to outlive the command, and a pack, a manifest or a range is in a folder
that is removed when the command ends. ADR 0029, ADR 0030.
**The source can change underneath us.** Size, mtime and inode (file id on Windows) are recorded at
import and checked again before serving starts. A change after that is caught by the receiver: the
ticket's root names the version that was hashed, so bytes read from the changed file do not verify
and the transfer fails with a `Verify` error. `--rehash-on-change` is not supported yet.

**Layout.**

```
$SLUICE_HOME/            XDG data dir, or Known Folders on Windows
  identity.key           Ed25519 secret, mode 0600
  VERSION                on-disk layout version
  db/                    tags, jobs, peer trust list
  last-port              UDP port of the last send, so a restart keeps its ticket working
  logs/
  tmp/                   private folders of running commands, swept when nobody holds them
```

Reclaimable content is capped at 10 GB by default, evicted least recently used unless tagged.
`sluice gc --dry-run` shows what would go. Completed destination files are never store-managed.

**Identity.** One Ed25519 key per user, created on first run. `sluice id` prints the endpoint id.
`sluice id --rotate` changes it, which breaks every pairing and every unexpired ticket naming the
old id, and the confirmation prompt says so.

**One writer per store.** On startup the process takes an exclusive lock holding its pid. If a live
process holds it, the CLI talks to that process over the control socket instead of opening the
store. A stale lock is reclaimed.

**Migration.** `VERSION` records the layout. Startup runs forward-only migrations and logs each one.
An older binary refuses a newer layout rather than writing to it.

## Connectivity

Two NATs cannot find each other without help: neither knows the other's mapped port. An iroh
endpoint coordinates a hole punch through its relay connection, where the two peers swap candidate
addresses and then send toward each other at once. Address discovery alone tells a peer its own
address and nobody else's, so it connects only a sender whose router accepts the first packet.

| Role | Bandwidth | Needed when |
|---|---|---|
| Discovery | none | Always, unless addresses are in the ticket |
| Relay as a meeting point | coordination, capped at 64 KiB/s per client | NAT to NAT across the internet |
| Relay as a full relay | the whole transfer | When hole punching fails, and always for the browser |

| Setup | Infrastructure | Direct connections |
|---|---|---|
| Same LAN | none, mDNS | all |
| Internet, sender's router supports PCP, NAT-PMP or UPnP-IGD | none | most |
| Internet, meeting point you run | one small VPS | about 90%, iroh's published figure with a relay, not yet measured by us; the rest run at 64 KiB/s |
| Internet, full relay you run | a VPS with bandwidth | all, at the relay's speed when not direct |

**Defaults.** Ticket addresses, mDNS and port mapping are on. n0's public relays, n0 DNS discovery
and the mainline DHT are off. In iroh terms that is `RelayMode::Disabled` until an operator names a
relay, then `RelayMode::Custom`. `RelayMode::Default` needs a flag and prints a warning. ADR 0011.

With no relay configured, LAN and port-mapped transfers work. A pair of symmetric NATs fails instead
of falling back, and the browser cannot connect at all. `sluice doctor` and the web UI's first
screen name the relay in use, or say there is none and what that rules out.

A sender keeps its endpoint id and remembers the UDP port it last bound in the home's `last-port`.
A restart tries that port first, so a ticket printed before it still dials the same direct address;
if the port is taken the sender takes any port and the ticket needs reprinting unless a relay is
configured. The home's identity key is why the id itself survives.

**One container, two recipes.** The `relay` image runs both halves of `iroh-relay`. As a meeting
point its per-client limit is 64 KiB/s, enough to coordinate and too little to carry files. As a
full relay the operator sets the limit from the host's bandwidth and carries the bill. A transfer
with no direct path 10 seconds in prints one line naming the relay and goes on. ADR 0028, which
retires the `rendezvous` image of ADR 0013.

**Relays run closed.** A leaked relay URL is free bandwidth for anyone, so our relay image ships
with authorization on. `open` mode needs a flag and a per-client byte rate limit; `iroh-relay` has
no working connection-accept limit, so byte limits are all we document. The image starts through
[`sluice-relay-guard`](../docker/guard/README.md), which refuses a config that would open more than
it says. ADR 0019, ADR 0020.

**Every transfer says how it is connected.** The progress line and `--json` output show direct or
relayed, and through which relay. A transfer that has no direct path ten seconds after its
connection came up says so once: `no direct path after 10 s; relayed through <url>`, then which
limits the two kinds of relay apply, because the client cannot tell a meeting point from a full
relay. Nothing fails over a relayed path; a direct path that arrives later is the ordinary
`path_changed`. `sluice doctor` is to report NAT type, port mapping result, MTU, UDP egress, the
chosen path, and the relay's reachability and authorization mode, in one block that can be pasted
into an issue. Today it checks the local home only, and reports its `transport` check as skipped
until the transport is measured; the [CLI README](../crates/sluice-cli/README.md) lists what it
checks.

**The browser is always relayed.** iroh in a tab has no UDP socket, so it reaches peers over a
WebSocket relay for the whole session. Browser throughput is relay throughput, and the web UI needs
a relay you run. The traffic stays end-to-end encrypted. ADR 0011, ADR 0014.

## Data path

QUIC carries discovery, hole punching, relay and control, and by default the data too. The settings
that matter for throughput:

- **Cubic** by default, with **BBRv3** selectable. ADR 0015 chose BBRv3, but `noq` 1.4.0's BBRv3
  has a max bandwidth bug (noq#852, fix quinn-rs/quinn#2481) that is not in a release yet, so
  ADR 0023 holds the default at Cubic until it is. Loss-based control cannot meet the lossy-path
  targets, so the switch back follows a measurement on S5, not only the release.
- **Windows sized to the bandwidth-delay product.** 1 Gbit/s at 100 ms needs 12.5 MB in flight. The
  defaults are far smaller.
- **GSO and GRO.** On by default in `noq`, but a failed probe falls back silently to one segment per
  call, which roughly halves throughput. The bench harness fails a run if that happened.
- **Several connections.** One QUIC connection is roughly one core of packet processing. Large
  transfers shard ranges across `min(cores / 2, 4)` connections. ADR 0016.
- **A TCP bulk path for direct peers.** When a direct route exists, data can move over several TCP
  streams with TLS (kTLS on Linux), plugged in through iroh's custom transport hook. QUIC keeps
  traversal and control. Whether this ships in 1.0 depends on the transport measurements. ADR 0016.

The disk side uses positioned reads and writes on a blocking pool, sparse files and `fallocate`.
NVMe outruns the packet path at 10 Gbit/s, so io_uring waits until a profile says otherwise (ADR
0007). `sendfile` does not apply, because QUIC encrypts in userspace.

BLAKE3 hashes at several GB/s per core with SIMD, and the outboard is cached so a repeat send never
rehashes. zstd (levels 1 to 3) runs only when a 64 KB sample compresses below 0.9, and never above 2
Gbit/s.

## Durability

A partial download lives in the per-transfer `iroh-blobs` store, which records verified ranges in its
own database and partial data files. A receive fetches every content blob before it writes the
first output file, so a lost connection is retried against the ranges the store already has and an
interrupted download leaves the output untouched. A file that a previous attempt did write is
recognised on resume by its content and is not treated as a clash. The process syncs the finished
file before it moves it into place. After a crash, the store decides what to ask for again from
its own record of verified ranges. This release adds no separate 64 MB barrier: `iroh-blobs`
commits its records as it goes, so after a crash the store may ask again for the ranges it
had not committed, which costs a re-read of part of the file, and no byte enters an output file
without verifying against the root. A crash can only cost work, never trust. A full disk, a
permission error or a quota error leaves the partial store and a `Storage` error, not a panic. A
test hook fails the copy of a packed file partway through, which is the same failure as a disk
that fills while bytes arrive, and the next receive finishes from the store it left.
The partial store is kept after a network failure, so a second receive with the same ticket and
`--out` continues from it, and it is removed once every file is in place.

## Errors

Every failure is one of six variants of `sluice_core::Error`. Callers decide on retries and exit
codes from the variant alone. The message is for people and can change in any release.

| Variant | Meaning | CLI exit code |
|---|---|---|
| `Network` | A peer or relay could not be reached, or a connection failed mid-transfer. Retrying can help | 3 |
| `Verify` | Received bytes did not match the BLAKE3 root. The bytes are discarded | 4 |
| `Storage` | The store or filesystem failed: full disk, permission or quota | 5 |
| `Policy` | Sluice refused by rule: the receiver declined, a path escapes the target directory, a size limit, a disallowed request such as an `iroh-blobs` push | 6 |
| `Cancelled` | The caller cancelled. Carries no message | 130 |
| `Bug` | An internal invariant broke. A defect in Sluice, never in the environment | 70 |

Exit code 2 stays with argument errors, as `clap` reports them. Bindings and the control API map the
same six variants to their own error types, one to one.

## Clients and bindings

| Client | Shape |
|---|---|
| CLI | The engine in process for one-shot commands, or a client of the running daemon. Human output unless `--json` is given, on a terminal or not. A terminal decides only colour, progress and whether to prompt |
| Desktop | Tauri 2 shell with the engine compiled in. One process, no sidecar |
| Browser | Svelte 5 PWA with the WASM build loaded when a transfer starts. Writes through the File System Access API on Chromium, OPFS elsewhere |
| MCP | `sluice mcp` over stdio, in the same binary |

The UI is one Svelte 5 codebase for both the browser and Tauri (ADR 0009). It has keyboard
navigation and WCAG 2.2 AA contrast from the first screen, and every string is externalized.

**Bindings live in `sdk/`, in this repository.** There are two surfaces. ADR 0017.

1. **In process.** A hand-written C ABI (`sluice-ffi`, with a `cbindgen` header) for C and C++. A
   UniFFI facade crate over it generates Python, Kotlin, Swift and Ruby, and Go and C# through
   NordSecurity's generators. Node uses napi-rs, the browser wasm-bindgen. Async calls and progress
   callbacks are part of every binding.
2. **Out of process.** Clients generated from `proto/sluice/v1` for every other language, talking to
   a running daemon over a local socket. MCP sits on this surface.

```
sdk/c/  sdk/python/  sdk/node/  sdk/kotlin/  sdk/swift/  sdk/go/  sdk/csharp/  sdk/java/
Package.swift      at the root, because SwiftPM requires it there; points into sdk/swift
```

Go tags carry the module path (`sdk/go/v1.2.3`) and the module ships prebuilt static libraries,
because `go get` cannot run cargo. All bindings share the core's version number.

## Crates

```
crates/
  sluice-core/    session, jobs, queue, events, tickets, manifest, packing, store, identity.
                  Wraps iroh and iroh-blobs. The only crate with hard async.
  sluice-live/    the live segment log. Sans-IO.
  sluice-cli/     the `sluice` binary, including `sluice mcp`.
```

Later phases add `sluice-delta` (sans-IO), `sluice-api` (the control service), `sluice-mount`
(fuse3, WinFsp, FSKit), `sluice-ffi`, `sluice-uniffi`, `sluice-node` and `sluice-wasm`. Each arrives
with the phase that needs it, along with its README.

The protocol crates take bytes and timer ticks in and return actions, with no IO of their own. That
makes them testable as plain functions, fuzzable from a byte slice, and reusable in the WASM build
without changes. ADR 0008.

## Testing

| What | How |
|---|---|
| Live log, delta, manifest, packing | Unit tests, `proptest`, `cargo-fuzz` on every parser |
| Resume and durability | `turmoil` with torn writes injected at every barrier; failures print a seed |
| Queue, scheduling, multi-provider | `turmoil` with slow, stalled and malicious providers |
| Real network | `netem` on bare metal: loss, latency, reordering, partitions, MTU black holes |
| Wire compatibility | `SPEC.md` test vectors through `cargo xtask conformance` |
| Interop | Current build against the previous and oldest supported releases |
| Whole system | A 100 GB file and a 100k-file tree, killed at random points, compared byte for byte |

The QUIC stack is tested by its own project, so we don't simulate QUIC.

## Tooling

| Concern | Tool |
|---|---|
| Toolchain | `rust-toolchain.toml`, Rust 2024 edition. MSRV 1.90 is one CI job, `cargo +1.90 check --all-targets --workspace --exclude sluice-probe`, not a matrix entry: `iroh` 1.4.0 and `iroh-blobs` 0.103.1 need Rust 1.91, and the probe is not a product crate |
| Tasks | `cargo xtask` |
| Tests and snapshots | `cargo nextest`, `insta` |
| Benchmarks | Planned, none run by a workflow: `divan` with CodSpeed for CPU work per pull request, and a self-hosted pair for throughput |
| Fuzzing | `cargo-fuzz` in `fuzz.yml`, on every pull request and nightly. OSS-Fuzz is planned |
| Lint | `clippy` and `rustfmt`, and `actionlint` and `shellcheck` on the workflows and their scripts, in `ci.yml`. `taplo` and `biome` are planned |
| Proto | Planned, no workflow runs them: `buf lint`, `buf breaking` |
| Dependencies | `cargo-deny` in `ci.yml`. `cargo-udeps` and Renovate are planned |
| API compatibility | Planned, no workflow runs it: `cargo-semver-checks` on published crates and `sluice-ffi` |
| Releases | `release-plz` for the version and the changelog pull request, `dist` 0.33.1 for the six archives and the two installers, from one tag through `release.yml`. The steps, the approvals and how to verify a release are in [the release runbook](runbooks/releasing.md) |
| Signing | GitHub artifact attestations: build provenance for every release file and image, and SBOMs. Azure Artifact Signing on Windows and notarization on macOS are planned and not done ([the release runbook](runbooks/releasing.md#not-done-yet)) |
| Containers | distroless, amd64 and arm64 built on native runners, each image starting through [`sluice-relay-guard`](../docker/guard/README.md) |
| Observability | Planned, no workflow runs it: `tracing`, optional OTLP export and Prometheus endpoint, both off by default. Today the log lines are there and neither export is built |
| CLI | `clap` with completions and man pages, `indicatif`, `NO_COLOR`, `--json` on every command |
