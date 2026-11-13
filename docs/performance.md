# Performance

Sluice aims to be the fastest way to move a file between two machines. This page says what that
means in numbers, how it is measured, and what each number is allowed to depend on. The design
choices behind the numbers are in [`design.md`](design.md#data-path).

A speed claim here is always scoped to a scenario below. There is no single "fastest" number.

## Targets

None of these has been measured yet. The transport is measured before any product code
([`roadmap.md`](roadmap.md)). If the stack cannot reach a target, the target is revised here with
the measurement attached.

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

Published measurements put userspace QUIC behind TCP at 10 Gbit/s, and the cost is receive-side
packet processing, not the kernel or the disk:

- TCP beat six QUIC implementations on 10 GbE by 16% to 297% ([KIT, IETF 118][kit]).
- Of four stacks, only the one using both GSO and GRO reached 8.3 Gbit/s ([TUM, NOMS 2026][tum]).

So the 8 Gbit/s target assumes several QUIC connections across cores, or the TCP bulk path (ADR
0016), and the single-connection target is set at 4 Gbit/s.

## At rest

Each of these is checked in CI on every pull request.

| Surface | Budget | How |
|---|---|---|
| `sluice` binary | under 12 MB stripped | The `dist` profile below, stripping when an archive is packaged, optional parts behind features, `cargo bloat` |
| Cold start to a printed ticket | under 30 ms | No work before it is needed: no config parse, store open or network probe a command does not use |
| `sluice serve` idle RSS | under 12 MB | No preallocated pools; the store opens lazily |
| `sluice serve` idle wakeups | 0 per second | Event-driven throughout, no polling |
| No process when idle | optional | systemd socket activation, launchd on demand, Windows delayed start |
| Web UI first load | under 60 KB of JavaScript | Svelte 5 runtime is 2 to 3 KB |
| WASM build | under 1.5 MB brotli | Loaded when a transfer starts, never at page load |
| Bindings | no background threads, no per-chunk allocation | They carry control and events only |
| Metrics and tracing exporters | off unless enabled | |
| Telemetry | none | There is no code that sends any |

### The release profile

`[profile.dist]`, which builds the released binaries, is measured rather than inherited. These
numbers are from one Windows machine, one clean build at a time: `cargo clean`, then `cargo build
--profile dist -p sluice-cli`, timed. The binary is the one cargo wrote, before the strip cargo-dist
does when it packages an archive.

| `[profile.dist]` | Binary | Clean build | `sluice id` | `sluice --version` | 1 GiB on loopback |
|---|---|---|---|---|---|
| `lto = "thin"` | 25,902,080 B (24.7 MiB) | 316 s | 29.8 ms | 23.4 ms | 9.36 s (9.62, 9.33, 9.36) |
| `lto = "fat"` | 21,446,144 B (20.5 MiB) | 426 s | 29.0 ms | 21.4 ms | 9.19 s (9.36, 9.19, 8.96) |
| `lto = "fat"`, `codegen-units = 1` | 18,521,600 B (17.7 MiB) | 435 s | 29.1 ms | 21.1 ms | 9.89 s (10.00, 9.89, 9.36) |

The profile is `lto = "fat"` with one codegen unit: the smallest binary of the three, for a clean
build a minute or two longer than thin LTO. That cost is paid once per release and not by a local
build, because `[profile.release]` keeps its defaults. Throughput was within the spread of the runs
for all three: the three runs of one profile differ from each other by more than the profiles differ
from each other, so the choice is on size and build time and not on speed. `panic = "abort"` would
be smaller still and cannot be used: `sluice-cli/src/bug.rs` catches a panic to report it as this
project's bug rather than as Rust's.

Cold start here was timed from PowerShell, which adds the shell's own process start to both ends, so
it is about ten milliseconds above what `cargo xtask cold-start` reports from Rust for a release
binary. The method is the same for all three, so the comparison holds. The transfer is one 1 GiB
generated file between a sender and a receiver on loopback, three runs per profile; both files were
hashed with SHA-256 and were equal in every run. The store's import was warm for the second and third
run of each profile, as it is for a second transfer of one file.

## How it is measured

CPU work (hashing, framing, manifests, packing, range sets) runs under `divan` on CodSpeed, which
counts instructions, so a 5% regression gate holds on shared CI runners.

Throughput cannot be measured that way. It runs on a self-hosted pair of bare-metal machines,
nightly and before every release. netem emulates the paths, because it misbehaves under nested
virtualization. Every published result carries:

- the `iperf3` ceiling on the same path, one stream and parallel
- the `fio` ceiling on both ends
- CPU, NIC, kernel, MTU, file system and exact command lines
- five runs, reported as median and spread
- the result as a percentage of the lower ceiling

Test data is generated from a seed, so no large files live in git. The harness and raw results are
in `bench/`, and anyone can rerun them.

The harness also asserts its own setup. iroh does not expose segment counts, but it counts the GRO
batches the socket received. If a Linux bench host counts none during a run, GRO did not run, and
the harness fails the run instead of publishing a number produced by a silent fallback.

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

- **Peer-to-peer tools:** croc, magic-wormhole on its own relay, sendme, DashBeam, LocalSend,
  Syncthing.
- **Bulk tools:** rclone, mscp, HPN-SSH, WDT, with rsync as the baseline.
- **Commercial tools** (Aspera, FileCatalyst) are not run. Their published S3 and S4 numbers are
  quoted with the conditions they state.

Every competitor runs its current release at its own recommended tuning. A comparison against an
untuned competitor is not published.

[kit]: https://datatracker.ietf.org/meeting/118/materials/slides-118-maprg-sustained-throughput-performance-of-quic-implementations
[tum]: https://www.net.in.tum.de/fileadmin/bibtex/publications/papers/spaethj-noms2026.pdf

## Delta receive

`sluice recv --delta-from PATH` rebuilds a file from an older one and fetches the rest. This is one
measurement of it, on one machine, and says nothing about a network.

**Setup.** Loopback, one sender process and one receiver process of the release binary, a file of
1 GiB (1,073,741,824 bytes) cut from a fixed seed, the older version held by the receiver. Five runs
of each row on a Windows 11 PC with an NVMe disk; the table gives the median and, in brackets, the
fastest and slowest run. Peak memory is the peak working set of each process, read from the operating
system after it exited. Every received file was compared byte for byte with the one sent. The target
of this page for memory, 250 MB, holds with room: 35 MiB for the receiver at most.

| Case | Receive | Fetched (of 1 GiB) | Signature bytes | Wall time, s | Peak memory, receiver / sender, MiB |
|---|---|---|---|---|---|
| 10 regions of 1 MiB changed in place (0.99%) | plain | 1,073,741,824 (100%) | 0 | 9.1 (8.8 to 12.3) | 27 / 26 |
| | delta | 10,656,413 (0.99%) | 2,276,888 | 8.5 (7.8 to 9.5) | 35 / 31 |
| 1 MiB inserted at the start | plain | 1,074,790,400 (100%) | 0 | 8.7 (7.9 to 11.7) | 26 / 26 |
| | delta | 1,058,975 (0.10%) | 2,278,808 | 6.6 (5.9 to 10.0) | 35 / 31 |
| Unrelated content | plain | 1,073,741,824 (100%) | 0 | 8.2 (7.6 to 14.8) | 27 / 26 |
| | delta | 1,073,741,824 (100%) | 2,275,288 (list read, plan dropped) | 12.0 (10.7 to 17.7) | 35 / 31 |

The unrelated-content rows come from a second set of five runs, and the other rows from the first.

**What it shows.** Both changed cases move well under 3% of the bytes: 0.99% for the edit in place,
which is the 10 MiB that changed and no more, and 0.10% for the insertion, which is the 1 MiB
inserted. The list for a 1 GiB file is 2.2 MB (0.21%), cut at the default 8 KiB average. On
loopback, where a plain receive already runs at disk speed, the delta receive takes about the same
time as a plain one for a changed file, and a few seconds more for content that shares nothing, which
is the cost of building and reading the list and cutting the candidate before the plain fetch runs.

**How the sender's list is built.** The sender reads the blob as a bao stream, a chunk group at a
time, on one task, and hashes and cuts it on a blocking thread. Reading 240 MiB took 1.0 s, about
230 MiB/s; the same read through the store's range export, which sends 1 KiB per message, ran at
36 MiB/s and made a 1 GiB list take half a minute. One list is built for each root, however many
receivers ask, and a receiver waits 60 s plus 1 s for every 64 MiB of the file for it.

**Not measured.** A file of 4 GiB. The run needs about 20 GB of disk: the sender's file, the
receiver's older file and its output, the store's copy and the assembly. At the measured speed the
list of a 4 GiB file builds in about 18 s, against a wait of 124 s, which is 60 s and 1 s for each
64 MiB of the file; that figure is a calculation and not a run. The harness is not part of the
repository, and the method above is enough to repeat it.
