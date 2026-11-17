<p align="center">
  <img src="assets/logo/sluice-128.png" width="128" alt="Sluice logo">
</p>

<h1 align="center">Sluice</h1>

<p align="center">Sluice sends a file or a folder straight from one computer to another. Nothing is stored in between.</p>

<p align="center"><a href="https://sluicedrop.com">sluicedrop.com</a></p>

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

Sluice moves a file or a folder from one computer to another without leaving a copy anywhere in
between. It is built for large files, slow links and connections that drop, and it checks every block
before it is written. There is no account, no hosted storage and no telemetry. A transfer goes
directly when the two machines can reach each other, and through a relay you run when they cannot.

## Three ways in

All three reach the same engine, and none is the main one. Pick the one that matches how you work.

### Send a file

The shortest way today is the page a local daemon serves. It needs no account, and every step is one
line to copy:

1. Install Sluice on both computers. [Every channel is below](#install).
2. On the computer that has the file, start the daemon and its page. `<folder>` is the one folder it
   may send from and receive into:

   ```sh
   sluice daemon --ui --root <folder>
   ```

3. In a second terminal, open the page:

   ```sh
   sluice ui
   ```

   It opens in your browser. `sluice ui --print` prints the address instead, for a browser on
   another machine or for a bookmark.

4. Press Send, then copy the ticket or show its QR code. On the other computer, run the same two
   commands, paste the ticket under Receive, look at what it offers, and choose where to save it.

A ticket is a secret: anyone who holds it can take the files while the transfer is open, so send it
the way you would send a password. The page talks only to the daemon on that computer, and the
address holds a key that only your user can read.

A desktop app, and sending from a browser with nothing installed, are coming in 1.2.
[The roadmap](docs/roadmap.md) says what each one waits on.

### For developers

The command line does the same thing without a daemon, and it is one binary for Linux, macOS and
Windows. [Install it](#install), then:

```sh
sluice send ./report.pdf          # prints a ticket, then serves the file
sluice inspect <ticket>           # what the ticket offers, and no content
sluice recv <ticket> --out ./inbox
```

The container image runs the same binary on Linux:

```sh
docker pull ghcr.io/sanamhub/sluice:0.1.0
```

[`crates/sluice-cli/README.md`](crates/sluice-cli/README.md) has every command, exit code and JSON
field. [Common tasks](#common-tasks) below has folders, live files, pipes, resume and receipts.

### For AI agents

`sluice mcp` is a Model Context Protocol server in the same binary. It hands the engine to an
assistant as seven tools over standard input and output (`send`, `receive`, `inspect`, `list_jobs`,
`job_status`, `cancel_job`, `doctor`), with no socket and no listener of its own. Add it to a client
and give it the folders it may use:

```json
{
  "mcpServers": {
    "sluice": {
      "command": "sluice",
      "args": ["mcp", "--root", "<folder>"]
    }
  }
}
```

A tool call starts a job and returns its id at once, so a long transfer does not hold a conversation
open. A path outside the folders the server was started with is refused, and text another computer
chose comes back in fields whose names begin `untrusted_`.

[sluicedrop.com/agents](https://sluicedrop.com/agents/) has the setup for common clients, the tool
list and what the server will not do. [`crates/sluice-cli/README.md`](crates/sluice-cli/README.md)
has the fields each tool takes and returns.

## Install

Linux and macOS:

```sh
curl --proto '=https' --tlsv1.2 -LsSf \
  https://github.com/sanamhub/sluice/releases/latest/download/sluice-cli-installer.sh | sh
```

Or with Homebrew:

```sh
brew install sanamhub/tap/sluice
```

On Windows, in PowerShell:

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

On Arch Linux, from the AUR:

```sh
yay -S sluice-bin
```

The container image is another channel, for Linux:

```sh
docker pull ghcr.io/sanamhub/sluice:0.1.0
```

[`docs/install.md`](docs/install.md) has every channel, the archive names, the checks that verify a
download, and the upgrade and uninstall command for each.

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

Check a receipt. It is the sender's signed statement that it offered some content at about the time
it names, and with `--file` the command also rebuilds the root a send of that file or folder would
name and checks it against the receipt's commitment and length:

```sh
sluice receipt verify receipt.txt --file ./report.pdf
```

It prints the commitment, the sender's key and the time. It cannot tell you that anyone received the
content or still holds it, and it says so.

Ask the sender for a receipt as part of a receive. The file is written once the whole transfer is in
place, and it is not replaced unless `--force` is given. A receipt path that is `--out` or inside it
is refused before anything is created, however it is spelled, including through a link, a junction
or a short name, so the receipt goes outside `--out`. A receipt that arrives and cannot be written
is a warning, and the receive still succeeds:

```sh
sluice recv <ticket> --out ./inbox --receipt ./receipt.txt
```

A send offers a receipt by default. `--no-receipt` clears the offer, and a receiver that asks is
answered with nothing:

```sh
sluice send ./photos --no-receipt
```

Fetch only what changed. Name the older version of the file, or a folder that holds files at the
ticket's own paths, and the parts that match are copied from it. The summary says how many bytes
came from the local file and how many from the network. The sender is told which ranges you still
need, so it can tell which parts of that file you hold: name only a file you are content to
disclose that of, and fetch plainly otherwise. The assembled file is written once more before it is
stored, so the volume needs room for the ticket's whole size as well, which is an upper bound:

```sh
sluice recv <ticket> --out ./inbox --delta-from ./inbox/report-v1.pdf
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
grants the same access as the original. `send --qr` draws it on the terminal as well, so a camera
pointed at that screen has the same access as a copy of the text.

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
installers, Homebrew, Scoop, winget, the AUR, and the tool and relay images. The desktop app, the
installers for a store, and sending from a browser with nothing installed are described in the
[roadmap](docs/roadmap.md) and do not exist yet.

Before 1.0 the command line and the wire format may change. [`SPEC.md`](SPEC.md) is the wire
contract, with test vectors for anyone writing a compatible implementation.

Contributions are welcome; start with [`CONTRIBUTING.md`](CONTRIBUTING.md). The project is
Apache-2.0 or MIT, at your option; see [`LICENSE-APACHE`](LICENSE-APACHE) and
[`LICENSE-MIT`](LICENSE-MIT).
