<p align="center">
  <img src="assets/logo/sluice-128.png" width="128" alt="Sluice logo">
</p>

<h1 align="center">Sluice</h1>

<p align="center">Sluice sends a file straight from one computer to another. Nothing is stored in between.</p>

<p align="center">
  <a href="https://github.com/sanamhub/sluice/actions/workflows/ci.yml">
    <img src="https://github.com/sanamhub/sluice/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://github.com/sanamhub/sluice/releases/latest">
    <img src="https://img.shields.io/github/v/release/sanamhub/sluice?sort=semver" alt="Latest release"></a>
  <a href="https://github.com/sanamhub/sluice/blob/main/LICENSE-MIT">
    <img src="https://img.shields.io/badge/license-Apache--2.0%20OR%20MIT-blue"
      alt="License: Apache-2.0 or MIT"></a>
  <img src="https://img.shields.io/badge/status-pre--1.0-orange" alt="Status: pre-1.0">
</p>

Sluice is a command-line tool for sending files and folders between two computers. It is built for
large files, slow links and connections that drop. There is no account, no hosted storage and no
telemetry. A transfer goes directly when the two machines can reach each other, and through a relay
you run when they cannot.

## Quick start

Install on Linux or macOS:

```sh
curl --proto '=https' --tlsv1.2 -LsSf \
  https://github.com/sanamhub/sluice/releases/latest/download/sluice-cli-installer.sh | sh
```

Or with Homebrew:

```sh
brew install sanamhub/tap/sluice
```

On Windows:

```powershell
irm https://github.com/sanamhub/sluice/releases/latest/download/sluice-cli-installer.ps1 | iex
```

Or with Scoop or winget:

```powershell
# Scoop
scoop bucket add sanamhub https://github.com/sanamhub/scoop-bucket
scoop install sluice

# winget
winget install --id sanamhub.sluice -e
```

The container image is a fourth way, for Linux:

```sh
docker pull ghcr.io/sanamhub/sluice:0.1.0
```

[`docs/install.md`](docs/install.md) has every channel, the archive names, the checks that verify a
download, and the upgrade and uninstall command for each.

Send a file from one machine:

```sh
sluice send ./report.pdf
```

It prints a ticket on one line and then serves the file. Send the ticket to the other machine, any
way you like, and run:

```sh
sluice inspect <ticket>
sluice recv <ticket> --out ./inbox
```

`inspect` prints what the ticket offers, with the number of files and folders, the size and the
names, and fetches no content. `recv` prints progress, verifies each block as it arrives, and ends
with a line that names the path it wrote. The sender stops after that one complete download.

## What makes it different

- **The file does not go through storage.** The sender reads it where it is, and the receiver writes
  it where it asks. No upload, no copy left on a server, and no third party in the path unless you
  configured a relay. The relay is a server you run, and it keeps nothing.
- **Every block is checked before it is written.** The file is split into content-addressed blocks,
  and a block that does not match its BLAKE3 root is discarded instead of written. A damaged
  transfer fails at the block, not after the file is opened.
- **A dropped transfer resumes.** A killed or disconnected receive keeps the ranges its store
  committed, so the next run fetches only the rest. No unverified byte is ever written.
- **A file that is still growing can be sent.** Live mode follows one file, seals fixed-size
  segments as they fill, and lets the receiver read each segment while the writer is still working.
- **You choose the relay.** No account, no plan limit, no telemetry, and no relay you did not name.
  With no relay configured, cases that need one fail and say why.

## Common tasks

Send a folder. It arrives inside a folder of its own name:

```sh
sluice send ./photos
sluice recv <ticket> --out ./inbox
```

Look at a ticket before fetching anything. `--all` lists past the first 50 names:

```sh
sluice inspect <ticket>
sluice inspect <ticket> --all
```

Take part of a folder. The pattern is matched against the ticket's own paths:

```sh
sluice inspect <ticket> --include 'photos/2025/**'
sluice recv <ticket> --include 'photos/2025/**' --out ./inbox
```

Follow a file that is still being written. A live ticket names no file, so `--out` is the file to
write:

```sh
sluice send --live ./recording.mkv
sluice recv <ticket> --out ./recording.mkv
```

Send a pipe. Standard input is copied into the store first, because its root is known only at the
end, so a pipe is never live:

```sh
tar c ./project | sluice send -
sluice recv <ticket> --out - > project.tar
```

Receive several tickets through one queue. Three run at once by default, and `--priority` decides
which starts first:

```sh
sluice recv <ticket-1> <ticket-2> <ticket-3> --out ./inbox --priority high
```

Cap the rate on either side. The units are `K`, `M`, `G`, `T` and `P` in powers of 1024, and `KB`,
`MB` and the like in powers of 1000:

```sh
sluice recv <ticket> --limit-rate 50M --out ./inbox
```

Share one ticket with two receivers. `--keep N` offers N places and stops after both places are
taken and their connections close:

```sh
sluice send --keep 2 ./dataset
```

Check a setup before a transfer. `doctor` reads only, prints one line per check with `ok`,
`warning`, `failed` or `skipped`, and exits nonzero only if a check failed:

```sh
sluice doctor
sluice doctor --network
```

The [CLI README](crates/sluice-cli/README.md) lists the exit codes, the JSON shapes and every
`doctor` check that scripts may rely on.

## How it works

Sluice uses [iroh](https://github.com/n0-computer/iroh) for QUIC, address discovery and hole
punching, and [iroh-blobs](https://github.com/n0-computer/iroh-blobs) for content-addressed
storage. A sender hashes a file into blocks, publishes the roots, and serves them. A manifest
carries names, sizes and modes, and a live transfer is a signed log of segments. The receiver
fetches what it needs, verifies every block against its root, and writes only verified bytes.
[`docs/design.md`](docs/design.md) has the architecture, the data model and the storage rules.

## Security

Every block of a fixed transfer is checked against its BLAKE3 root before it is written, and a live
segment is checked against the signed log before its frame is accepted. Paths in the manifest are
checked before anything is created: no absolute path, no `..`, and no link followed outside the
output folder. The transport is QUIC with TLS 1.3, and the handshake prefers a post-quantum key
exchange.

A ticket is a read capability. It names the sender's endpoint id, the addresses the sender can be
reached at, the relay if one is used, the content root or live key, and the expiry. It does not
contain file names. Anyone who has the ticket can fetch the content while the sender is serving it
and the ticket has not expired, so treat it like a password. An unexpired ticket that is forwarded
grants the same access as the original.

[`docs/threat-model.md`](docs/threat-model.md) lists what is protected, what is not, and the known
weaknesses. Report a vulnerability privately through GitHub Security Advisories as
[`SECURITY.md`](SECURITY.md) says, never in a public issue.

## Run your own relay

The relay image is `ghcr.io/sanamhub/sluice-relay`. It runs as a meeting point, which coordinates a
hole punch at 64 KiB/s per client and carries no file data, or as a full relay, which carries the
bytes for pairs that cannot connect directly. It ships closed, with an empty allowlist, so nobody
uses it until you add an endpoint id.

```sh
docker pull ghcr.io/sanamhub/sluice-relay:0.1.0
```

[`docker/relay/README.md`](docker/relay/README.md) has the certificate, both recipes, the allowlist
and the guard. [`docker/cli/README.md`](docker/cli/README.md) covers the tool image, its home volume
and its network modes.

## Project status

0.1.0 is the first release. It ships the `sluice` binary for six targets, the shell and PowerShell
installers, Homebrew, Scoop, winget, the AUR, and the tool and relay images. The desktop and browser
clients described in the [roadmap](docs/roadmap.md) do not exist yet.

Before 1.0 the command line and the wire format may change. [`SPEC.md`](SPEC.md) is the wire
contract, with test vectors for anyone writing a compatible implementation.

Contributions are welcome; start with [`CONTRIBUTING.md`](CONTRIBUTING.md). The project is
Apache-2.0 or MIT, at your option; see [`LICENSE-APACHE`](LICENSE-APACHE) and
[`LICENSE-MIT`](LICENSE-MIT).
