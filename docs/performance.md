# Performance

Sluice aims to be the fastest way to move a file between two machines. This page says what that means
in numbers, how it is measured, and what each number is allowed to depend on. The design choices behind
the numbers are in [`design.md`](design.md#data-path).

A speed claim here is always scoped to a scenario below. There is no single "fastest" number.

## Targets

None of these has been measured yet. M0 measures the transport first ([`roadmap.md`](roadmap.md)). If
the stack cannot reach a target, the target is revised here with the measurement attached.

| Scenario | Target |
|---|---|
| 10 GbE LAN, NVMe to NVMe, one 100 GB file | 8 Gbit/s or more, within 10% of `iperf3` |
| One QUIC connection, same LAN | 4 Gbit/s or more |
| 1 Gbit/s, 100 ms RTT, 0.1% loss | 1 Gbit/s or more |
| 1 Gbit/s, 200 ms RTT, 2% loss | 300 Mbit/s or more |
| 100k files averaging 20 KB | 60% or more of the throughput of one file of the same total size |
| Resume after `kill -9` at 60% of 100 GB | First new byte within 2 s; at most one 64 MB barrier re-verified |
| Delta resend, 50 GB with 1% changed | Under 3% of the bytes on the wire |
| Live mode, from write to readable on the receiver | Under 1 s at the default segment size |
| Mount, sequential reads with 1 MiB readahead, LAN | 500 MB/s or more |
| Browser to native, 20 GB, through a relay you run | 500 Mbit/s or more, published with the relay's CPU, NIC and egress |
| Memory during any transfer | Under 250 MB RSS |

### Why the 10 GbE target needs more than one connection

Published measurements put userspace QUIC behind TCP at 10 Gbit/s, and the cost is receive-side packet
processing, not the kernel or the disk:

- TCP beat six QUIC implementations on 10 GbE by 16% to 297% ([KIT, IETF 118](https://datatracker.ietf.org/meeting/118/materials/slides-118-maprg-sustained-throughput-performance-of-quic-implementations)).
- Of four stacks, only the one using both GSO and GRO reached 8.3 Gbit/s ([TUM, NOMS 2026](https://www.net.in.tum.de/fileadmin/bibtex/publications/papers/spaethj-noms2026.pdf)).

So the 8 Gbit/s target assumes several QUIC connections across cores, or the TCP bulk path (ADR 0016),
and the single-connection target is set at 4 Gbit/s.

## At rest

Each of these is checked in CI on every pull request.

| Surface | Budget | How |
|---|---|---|
| `sluice` binary | under 12 MB stripped | LTO, `panic = "abort"`, optional parts behind features, `cargo bloat` |
| Cold start to a printed ticket | under 30 ms | No work before it is needed: no config parse, store open or network probe a command does not use |
| `sluice serve` idle RSS | under 12 MB | No preallocated pools; the store opens lazily |
| `sluice serve` idle wakeups | 0 per second | Event-driven throughout, no polling |
| No process when idle | optional | systemd socket activation, launchd on demand, Windows delayed start |
| Web UI first load | under 60 KB of JavaScript | Svelte 5 runtime is 2 to 3 KB |
| WASM build | under 1.5 MB brotli | Loaded when a transfer starts, never at page load |
| Bindings | no background threads, no per-chunk allocation | They carry control and events only |
| Metrics and tracing exporters | off unless enabled | |
| Telemetry | none | There is no code that sends any |

## How it is measured

CPU work (hashing, framing, manifests, packing, range sets) runs under `divan` on CodSpeed, which counts
instructions, so a 5% regression gate holds on shared CI runners.

Throughput cannot be measured that way. It runs on a self-hosted pair of bare-metal machines, nightly
and before every release. netem emulates the paths, because it misbehaves under nested
virtualization. Every published result carries:

- the `iperf3` ceiling on the same path, one stream and parallel
- the `fio` ceiling on both ends
- CPU, NIC, kernel, MTU, file system and exact command lines
- five runs, reported as median and spread
- the result as a percentage of the lower ceiling

Test data is generated from a seed, so no large files live in git. The harness and raw results are in
`bench/`, and anyone can rerun them.

The harness also asserts its own setup. iroh does not expose segment counts, but it counts the GRO
batches the socket received. If a Linux bench host counts none during a run, GRO did not run, and the
harness fails the run instead of publishing a number produced by a silent fallback.

### Scenarios

| # | Link | RTT | Loss | Purpose |
|---|---|---|---|---|
| S1 | 10 Gbit/s LAN, NVMe | 0.2 ms | 0 | CPU and I/O ceiling |
| S2 | 1 Gbit/s | 20 ms | 0 | Regional |
| S3 | 1 Gbit/s | 80 ms | 0.1% | Cross-continent; matches FileCatalyst's published case |
| S4 | 1 Gbit/s | 170 ms | 0.001% | Matches Aspera's published case |
| S5 | 100 Mbit/s | 200 ms | 1 to 5% | Lossy long haul |
| S6 | 10 Gbit/s | 60 ms | 0.1% | High bandwidth-delay product |
| S7 | 20 to 100 Mbit/s | 40 ms ± 10 ms | 0.5% | Wi-Fi and LTE |
| S8 | any | any | any | Direct after hole punching, compared with a forced relay |
| S9 | any | any | any | Time from a link cut to resumed transfer |

Each scenario runs three workloads: one file of 10 to 100 GB, 100k files of 4 to 64 KB, and a mixed
tree.

### Compared against

- **Peer-to-peer tools:** croc, magic-wormhole on its own relay, sendme, DashBeam, LocalSend, Syncthing.
- **Bulk tools:** rclone, mscp, HPN-SSH, WDT, with rsync as the baseline.
- **Commercial tools** (Aspera, FileCatalyst) are not run. Their published S3 and S4 numbers are quoted
  with the conditions they state.

Every competitor runs its current release at its own recommended tuning. A comparison against an
untuned competitor is not published.
