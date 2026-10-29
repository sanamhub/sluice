<p align="center">
  <img src="assets/logo/sluice-128.png" width="128" alt="Sluice logo">
</p>

<h1 align="center">Sluice</h1>

<p align="center">
  <a href="https://github.com/sanamhub/sluice/actions/workflows/ci.yml">
    <img src="https://github.com/sanamhub/sluice/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://github.com/sanamhub/sluice/blob/main/LICENSE-MIT">
    <img src="https://img.shields.io/badge/license-Apache--2.0%20OR%20MIT-blue"
      alt="License: Apache-2.0 or MIT"></a>
  <img src="https://img.shields.io/badge/status-pre--1.0-orange" alt="Status: pre-1.0">
</p>

Sluice sends a file straight from one computer to another. Nothing is stored in between.

It is built for large files and slow links. Every piece of the file is checked as it arrives, and a
dropped transfer picks up where it stopped. There is no account, no hosted service, no size plan,
and no server in the middle that you did not choose.

> **Status:** pre-1.0. `sluice send`, `sluice recv`, `sluice inspect`, `sluice id`, `sluice doctor`
> and `sluice completions` work today, including live mode, several tickets through the queue, a
> byte range, pipes and a per-command byte cap. The desktop and browser clients do not exist yet.

## Why it is different

- **The file goes where it is needed.** There is no upload to a storage service and no copy left on
  a server. A relay is a server you run that can forward data when a direct path fails, and it does
  not keep them.
- **Every piece is checked.** The file is split into small blocks. A damaged block is caught before
  it is written, not after the file is opened.
- **You own the path.** No account, no plan limit, no telemetry, and no relay you did not configure.
  The relay image is in this repository.

## Try it

There is no release yet, so build from source. A transfer between two folders on one machine, or
two machines that can reach each other, works today:

```sh
cargo run -p sluice-cli -- send ./bigfile.mkv    # prints a ticket
cargo run -p sluice-cli -- recv <ticket> --out ./out
cargo run -p sluice-cli -- send --live ./recording.mkv   # a file that is still being written
cargo run -p sluice-cli -- recv <ticket> --out recording.mkv  # a live ticket names no file
cargo run -p sluice-cli -- recv <t1> <t2> <t3> --out ./out --priority high  # three at once
cargo run -p sluice-cli -- recv <ticket> --limit-rate 50M --out ./out
cat dump.sql | cargo run -p sluice-cli -- send - # a pipe is copied into the store first
cargo run -p sluice-cli -- recv <ticket> --out - # a single file to standard output
cargo run -p sluice-cli -- inspect <ticket>       # what it offers: names, counts and size

cargo run -p sluice-cli -- id
cargo run -p sluice-cli -- doctor
cargo run -p sluice-cli -- doctor --network           # UDP, relay, NAT and port mapping
cargo run -p sluice-cli -- completions bash
```

A folder arrives inside a folder of its own name, as copying it in a file manager does. A name
already in the output folder refuses the whole transfer unless `--force`, a top-level name that
starts with `.` needs `--allow-hidden`, and a link is replaced as an entry and never followed. A
folder is never replaced by a file or a link, even with `--force`, and the error says so. A receive
that was killed resumes from its partial store only when that store was made for the same ticket;
a file whose content already matches is skipped, and anything else at its target is a clash.

`doctor` changes nothing. It prints one line per check, `ok`, `warning`, `failed` or `skipped`, and
the step that fixes a problem under it. Its exit code is 0 unless a check failed. With `--network`
it also binds a UDP socket, asks the configured relay for a net report, and reports the public
address, the NAT mapping and whether a router can open a port; without a relay it says that two
hard NATs cannot connect. With `--json` the same report is one object. The
[CLI README](crates/sluice-cli/README.md) lists every check, exit code and JSON field, which scripts
may rely on.

`sluice id --rotate` replaces the identity after asking. With no terminal, or with `--json`, it
refuses unless `--yes` is given.

## Install

There is no release yet. When there is one, [`docs/install.md`](docs/install.md) lists the six
archive names and the commands that check where an archive came from before it is run. The
installers in a release check less than that, and the same page says what each one does.

## More of 1.0

Live mode, ranges and several tickets through the queue work today.

```sh
sluice recv <ticket> --range 4G:4.1G    # fetch and verify one byte range
sluice recv <ticket> --include 'photos/**'   # take part of a folder
sluice send --live ./recording.mkv      # follow a file that is still being written
```

`--include` takes a glob over the ticket's own paths, may be given more than once, and
`sluice inspect --include` shows the same part before anything is fetched.

`--range` takes a half-open range, `start:end`, `start:+length`, `start:` or `:end`, in bytes or
with a unit: `K`, `M`, `G`, `T` and `P` (and `KiB` and the like) count in 1024s, `KB`, `MB` and the
like in 1000s, and a decimal such as `4.1G` is rounded down to a whole byte.

A recording or a database dump is readable on the other side while it is still being written,
verified segment by segment; the sender closes the stream after two seconds without growth, or the
time `--close-after` names. A single byte range can be fetched and verified without the rest. A
transfer that is killed, unplugged or runs out of disk resumes from the ranges its store committed:
`iroh-blobs` commits them in batches, so a crash can cost a re-read of the part that was not
committed, and no unverified byte is ever written.

The speed target is 8 Gbit/s on a 10 GbE LAN, within 10% of `iperf3`, for one 100 GB file. The
lossy-path target and the resume budget are on the [performance page](docs/performance.md). None of
these has been measured on a real network yet; the loopback numbers in the engine report are a
local check, not a network measurement. Every number will be published with the hardware, the
network conditions and a script to rerun it.

Files are hashed where they are. Sending a 100 GB file stores about 0.4% of its size in outboard
data rather than copying it; `--copy` imports it instead. The engine is a library as well as a
binary, under Apache-2.0 or MIT. Bindings for Python, Node, Swift, Kotlin, Go, C# and C, a local
control API and an MCP server are planned in this repository.

## Connectivity

Two computers behind NATs need help to find each other. Across the internet that help is a relay
you run: the two ends coordinate a hole punch through it, and the file then goes direct.

| Where | What you need |
|---|---|
| Same network | Nothing |
| Internet, and the sender's router supports PCP, NAT-PMP or UPnP | Nothing |
| Internet, otherwise | A relay you run as a meeting point. It fits on the smallest VPS, and when hole punching works it carries no file data |
| Internet, when hole punching fails | A relay you run as a full relay, which carries the data |

Sluice never falls back to someone else's relay. When no relay is configured, the cases that need
one fail and say why. The browser build is always relayed, because iroh in a tab has no UDP socket,
so it needs a relay you run as well. [ADR 0011](docs/adr/0011-browser-is-relay-bound.md) and
[ADR 0028](docs/adr/0028-one-relay-image-two-recipes.md).

## Run your own

One image, `relay`, run one of two ways. As a meeting point it limits each client to 64 KiB/s, so
it coordinates hole punches and is too slow to carry files. As a full relay it carries the
transfers that cannot go direct, at a limit you set. It ships with an allowlist, so nobody uses it
until you add an endpoint id, and it starts through a guard that refuses a config that would open it
by mistake.

- [`docker/relay/README.md`](docker/relay/README.md) has the certificate, the build, the
  `docker run` for both ways, the allowlist and open mode.
- [`docker/guard/README.md`](docker/guard/README.md) lists every rule the guard enforces.

## Built on

[iroh](https://github.com/n0-computer/iroh) for connectivity and QUIC, and
[iroh-blobs](https://github.com/n0-computer/iroh-blobs) for content-addressed, verified streaming.
Sluice adds live transfer, delta hints, directory manifests and packing, the parallel data path, and
the CLI, bindings and UI. Live transfer works on the same principle as
[Hypercore](https://github.com/holepunchto/hypercore)'s signed logs, applied to plain files. ADR
0004 covers the differences.

## Documentation

| | |
|---|---|
| [`docs/design.md`](docs/design.md) | Architecture, data model, storage, connectivity |
| [`SPEC.md`](SPEC.md) | Wire protocol, with test vectors, for anyone writing a compatible implementation |
| [`docs/performance.md`](docs/performance.md) | Targets and how they are measured |
| [`docs/threat-model.md`](docs/threat-model.md) | What is and is not protected |
| [`docs/roadmap.md`](docs/roadmap.md) | 1.0, what comes after, and what will not be built |
| [`docs/adr/`](docs/adr/README.md) | Decision records |
| [`crates/sluice-cli/README.md`](crates/sluice-cli/README.md) | The commands, their exit codes and their JSON |
| [`docs/install.md`](docs/install.md) | How to install and verify a release |
| [`docs/runbooks/releasing.md`](docs/runbooks/releasing.md) | How a release is made |

## Contributing and security

Start with [`CONTRIBUTING.md`](CONTRIBUTING.md). Report vulnerabilities privately as described in
[`SECURITY.md`](SECURITY.md), never in a public issue.

## License

Apache-2.0 or MIT, at your option. Sluice collects no telemetry.
