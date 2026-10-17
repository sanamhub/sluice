# Sluice

Peer-to-peer file transfer that saturates the link. No cloud, no accounts, no third-party service.
Direct device to device over QUIC. Any rendezvous or relay is one you run.

**Status: pre-implementation.** [`plan.md`](plan.md) is the design, [`SPEC.md`](SPEC.md) is the protocol.
Neither is stable yet. There is no code.

License: Apache-2.0 OR MIT. Free forever. No telemetry, ever.

---

## What it will do

```
sluice send ./bigfile.mkv               # prints a ticket
sluice send --live ./recording.mkv      # send a file that is still being written
cat dump.sql | sluice send -            # send a pipe
sluice recv <ticket> -o ./out
sluice recv <ticket> --range 4G:4.1G    # verified partial fetch
sluice mount <ticket> /mnt/remote       # remote content as a read-only path
sluice doctor                           # NAT class, MTU, path in use, why
```

## Why another one

| Tool | Gap it leaves |
|---|---|
| Blip | Closed source, paid, their relays |
| DashBeam | AGPL, so nothing can embed it. No live streaming, no ranges, no API |
| croc, magic-wormhole | Relay-bound, no hole punching, slow |
| LocalSend | One LAN only |
| Syncthing | Folder sync, not one-shot send |
| Aspera | Six figures, closed |

What none of them have, together:

- **Live streaming.** Read a plain file that is still being written, verified as it arrives.
- **Verified random access.** Fetch and verify byte range 4G to 4.1G of a 200 GB file.
- **A real API.** A `--json` CLI, plus gRPC, Connect, SDKs, and an MCP adapter over the same calls.
  Bytes never pass through any of them.
- **Published benchmarks.** Method, hardware, netem profiles, raw data, rerun script. They cover WAN,
  NAT, and relayed paths, not only LAN.
- **Apache-2.0.** Other products can embed the engine.

None of these is new on its own, and we say where each comes from:

- [Hypercore](https://github.com/holepunchto/hypercore) and Hyperdrive have signed, live, verifiable
  append-only logs and byte ranges. Our live mode has the same shape, for plain
  files, closing into an ordinary BLAKE3 root.
- `iroh-blobs` already does verified ranges.
- [`ffl-mcp`](https://github.com/nuwainfo/ffl-mcp) already lets agents send files over MCP.

What is new is having all of them in one embeddable transfer engine.

## What it is built on

`iroh` and `noq` for QUIC, NAT traversal, and relay fallback. `iroh-blobs` and `bao-tree` for
content-addressed verified streaming.

We build live streaming, delta hints, manifests and packing, the session layer, and the clients.
Everything else is a dependency, not a sprint. See `plan.md` section 3.

## Honest connectivity

You cannot punch between two NATs with zero infrastructure. On a LAN, Sluice needs no server. On the
internet it needs either a router that speaks PCP, NAT-PMP, or UPnP-IGD, or a rendezvous you run, which
fits on the smallest VPS available. About 10% of paths also need a relay, per iroh's published
hole-punching rate.

`plan.md` section 6 has the full matrix, and `sluice doctor` tells you which case you are in. Every
transfer also shows, while it runs, whether it is direct or relayed and through which relay.

The browser is the exception. Sluice's browser build runs iroh, and iroh in a tab has no UDP socket,
so it reaches peers only through a WebSocket relay. **Every Sluice browser transfer is relayed for its
whole life**, end to end encrypted but passing through a relay you run.

This is a limit of our stack, not of browsers. WebRTC tools such as FilePizza can hole-punch from a
tab, and `plan.md` section 10.1 explains why we do not add a second connectivity stack to get that. If
you want the web UI, plan for a relay. If you only want the CLI, you may never need one.

## Repository

```
crates/     the Rust engine and CLI
src/        the optional C# hub and SDK
proto/      the one API contract, governed by buf
ui/         one Svelte 5 app, built as a PWA and as a Tauri shell
bench/      benchmark harness and published results
docs/adr/   architecture decision records
docs/research/  evidence notes, with the source of every claim
SPEC.md     wire protocol, written before the code
plan.md     the full design, including what we deliberately do not build
```

## Contributing

Read [`CONTRIBUTING.md`](CONTRIBUTING.md). The short version: `cargo nextest run` works from a clean
clone with no network, no daemon, and no .NET installed. Every crate README says what that crate owns
and what it must never do. Every non-obvious decision has an ADR.

## Security

Read [`SECURITY.md`](SECURITY.md). Report vulnerabilities privately, never in a public issue.
