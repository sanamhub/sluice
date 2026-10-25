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

Peer-to-peer file transfer over QUIC, with every 16 KiB block checked against a BLAKE3 root before
it is written. On a 10 GbE LAN the target is 8 Gbit/s, within 10% of `iperf3`, for one 100 GB file.
That is a target, not a measurement: [`docs/performance.md`](docs/performance.md) says how each
number will be measured and published. No account, no hosted service, and no relay you did not
configure.

> **Status:** pre-1.0. `sluice id`, `sluice doctor` and `sluice completions` work today. The 1.0
> commands below are planned and do not exist yet.

## What works today

```
sluice id                      # print this machine's endpoint id
sluice id --rotate             # replace the identity, after asking
sluice doctor                  # check the home, its lock, key and free space
sluice completions bash        # print a shell's completion script
```

`sluice doctor` changes nothing. It prints one line per check, `ok`, `warning`, `failed` or
`skipped`, with the step that fixes a problem under it, and `--json` gives the same as one object.
Its exit code is 0 unless a check failed, and the `transport` check is skipped until the transport
is measured. The [CLI README](crates/sluice-cli/README.md) lists every check, the exit codes and the
JSON, which scripts may rely on.

## Planned for 1.0

These commands do not exist yet. The parser behind `--range` does; it is tested and fuzzed, and
lives in `sluice-core`.

```
sluice send ./bigfile.mkv               # prints a ticket
sluice recv <ticket> -o ./out

sluice send --live ./recording.mkv      # send a file while it is still being written
cat dump.sql | sluice send -            # send a pipe
sluice recv <ticket> --range 4G:4.1G    # fetch and verify one byte range
```

`--range` takes a half-open range, `start:end`, `start:+length`, `start:` or `:end`, in bytes or
with a unit: `K`, `M`, `G`, `T` and `P` (and `KiB` and the like) count in 1024s, `KB`, `MB` and the
like in 1000s, and a decimal such as `4.1G` is rounded down to a whole byte.

## Install

There is no release yet. When there is one, [`docs/install.md`](docs/install.md) lists the six
archive names and the commands that check where an archive came from before it is run. The
installers in a release check less than that, and the same page says what each one does.

## What sets it apart

The speed claim comes with a method. Every number is published with the hardware, the network
conditions and a script to rerun it. The 10 GbE target, the lossy-path target and the resume budget
are in [`docs/performance.md`](docs/performance.md); none has been measured yet.

A recording or a database dump is readable on the other side while it is still being written,
verified segment by segment. A single byte range can be fetched and verified without the rest.
A transfer that is killed, unplugged or runs out of disk resumes after its last verified range, and
at most one 64 MB barrier is checked twice.

Files are hashed where they are. Sending a 100 GB file stores about 0.4% of its size in outboard
data rather than copying it; `--copy` imports it instead. The engine is a library as well as a
binary, under Apache-2.0 or MIT. Bindings for Python, Node, Swift, Kotlin, Go, C# and C, a local
control API and an MCP server are planned in this repository.

## Connectivity

Two machines behind NATs need help to find each other.

| Where | What you need |
|---|---|
| Same network | Nothing |
| Internet, and either router supports PCP, NAT-PMP or UPnP | Nothing |
| Internet, otherwise | A rendezvous server you run. It fits on the smallest VPS and carries no file data |
| Internet, when hole punching fails | A relay you run, which does carry the data |

Sluice never falls back to someone else's relay. When no relay is configured, the cases that need
one fail and say why. The browser build is always relayed, because iroh in a tab has no UDP socket,
so it needs a relay you run as well. ADR 0011 and ADR 0013.

## Run your own

The `rendezvous` image answers address discovery and moves no transfer bytes. The `relay` image
carries the traffic and ships with an allowlist, so nobody relays until you add an endpoint id. Both
run through a guard that refuses a config that would open them by mistake.

- [`docker/rendezvous/README.md`](docker/rendezvous/README.md) has the certificate, the build and
  the `docker run`.
- [`docker/relay/README.md`](docker/relay/README.md) has the same, plus the allowlist and open mode.
- [`docker/guard/README.md`](docker/guard/README.md) lists every rule the guard enforces.

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

## Built on

[iroh](https://github.com/n0-computer/iroh) for connectivity and QUIC, and
[iroh-blobs](https://github.com/n0-computer/iroh-blobs) for content-addressed, verified streaming.
Sluice adds live transfer, delta hints, directory manifests and packing, the parallel data path, and
the CLI, bindings and UI. Live transfer works on the same principle as
[Hypercore](https://github.com/holepunchto/hypercore)'s signed logs, applied to plain files. ADR
0004 covers the differences.

## Contributing and security

Start with [`CONTRIBUTING.md`](CONTRIBUTING.md). Report vulnerabilities privately as described in
[`SECURITY.md`](SECURITY.md), never in a public issue.

## License

Apache-2.0 or MIT, at your option. Sluice collects no telemetry.
