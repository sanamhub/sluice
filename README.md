# Sluice

Peer-to-peer file transfer built to fill the link. Files go directly from one machine to another
over QUIC, verified chunk by chunk, with no account and no service in the middle that you don't run
yourself.

**Status:** before 1.0, which [`docs/roadmap.md`](docs/roadmap.md) describes. The `sluice` binary
has three commands today:

```
sluice id                               # print this machine's endpoint id
sluice id --rotate                      # replace the identity, after asking
sluice doctor                           # check the home, who can reach it, its lock, key and space
sluice completions bash                 # print a shell's completion script
```

`sluice doctor` changes nothing. It prints one line per check, `ok`, `warning`, `failed` or
`skipped`, with the step that fixes a problem under it, and `--json` gives the same as one object.
Its exit code is 0 unless a check failed. The `transport` check, of NAT type, path and relay, is
skipped until the M0 measurements. The [CLI README](crates/sluice-cli/README.md) lists every check,
the exit codes and the JSON, which scripts may rely on.

What 1.0 adds, and does not exist yet:

```
sluice send ./bigfile.mkv               # prints a ticket
sluice recv <ticket> -o ./out

sluice send --live ./recording.mkv      # send a file while it is still being written
cat dump.sql | sluice send -            # send a pipe
sluice recv <ticket> --range 4G:4.1G    # fetch and verify one byte range
```

`--range` takes a half-open range, `start:end`, `start:+length`, `start:` or `:end`, in bytes or
with a unit: `K`, `M`, `G`, `T` and `P` (and `KiB` and the like) count in 1024s, `KB`, `MB` and the
like in 1000s, and a decimal such as `4.1G` is rounded down to a whole byte. The parser is in
`sluice-core` (`range`) and is tested and fuzzed; `sluice recv`, which will take it, is not built
yet.

## What 1.0 does

- **Fills fast links.** The 10 GbE target is within 10% of `iperf3`. Large transfers spread across
  several connections, and BBRv3 handles lossy long-distance paths. Every number is published with
  the hardware, the network conditions and a script to rerun it. See
  [`docs/performance.md`](docs/performance.md).
- **Verifies everything.** Content is addressed by BLAKE3, and every 16 KiB is checked before it is
  written or returned.
- **Resumes anywhere.** Kill it, unplug the cable, fill the disk. The next run picks up after the
  last verified range.
- **Sends files that are still growing.** A recording or a dump can be read on the other side while
  it is being written, verified segment by segment.
- **Reads any range.** Fetch and verify bytes 4G to 4.1G of a 200 GB file without the rest.
- **Leaves your files where they are.** Sending a 100 GB file doesn't copy it anywhere first.
- **Embeds.** Apache-2.0 or MIT. Bindings for Python, Node, Swift, Kotlin, Go, C# and C are planned
  in this repository, with a local API and an MCP server for everything else.

## Connectivity

Two machines behind NATs need help to find each other.

| Where | What you need |
|---|---|
| Same network | Nothing |
| Internet, and either router supports PCP, NAT-PMP or UPnP | Nothing |
| Internet, otherwise | A rendezvous server you run. It fits on the smallest VPS and carries no file data |
| Internet, when hole punching fails | A relay you run, which does carry the data |

Sluice never falls back to someone else's relay. When no relay is configured, the cases that need
one fail and say why. Once its transport check is built, after M0, `sluice doctor` will tell you
which case you are in, and every transfer will show whether it went direct or through a relay.

**The browser version is always relayed.** iroh in a browser tab has no UDP socket, so it reaches
peers only through a relay. The relay can't read the traffic, but it does carry every byte, so the
web UI needs a relay you run.

## Built on

[iroh](https://github.com/n0-computer/iroh) for connectivity and QUIC, and
[iroh-blobs](https://github.com/n0-computer/iroh-blobs) for content-addressed, verified streaming.
Sluice adds:

- live transfer of growing files
- delta transfers
- directory manifests and small-file packing
- the parallel data path
- the CLI, the bindings and the UI

How it fits together is in [`docs/design.md`](docs/design.md). Live transfer works on the same
principle as [Hypercore](https://github.com/holepunchto/hypercore)'s signed logs, applied to plain
files; ADR 0004 explains the differences.

## Documentation

| | |
|---|---|
| [`docs/design.md`](docs/design.md) | Architecture, data model, storage, connectivity |
| [`SPEC.md`](SPEC.md) | Wire protocol, with test vectors, for anyone writing a compatible implementation |
| [`docs/performance.md`](docs/performance.md) | Targets and how they are measured |
| [`docs/threat-model.md`](docs/threat-model.md) | What is and isn't protected |
| [`docs/roadmap.md`](docs/roadmap.md) | Phases, open questions, what won't be built |
| [`docs/adr/`](docs/adr/README.md) | Decision records |
| [`crates/sluice-cli/README.md`](crates/sluice-cli/README.md) | The commands, their exit codes and their JSON |
| [`RELEASING.md`](RELEASING.md) | How a release is made, and how to verify one |
| [`docker/`](docker/relay/README.md) | The `relay` and `rendezvous` images, and the guard they start through |

## Contributing and security

Start with [`CONTRIBUTING.md`](CONTRIBUTING.md). Report vulnerabilities privately as described in
[`SECURITY.md`](SECURITY.md), never in a public issue.

## License

Apache-2.0 or MIT, at your option. Sluice collects no telemetry.
