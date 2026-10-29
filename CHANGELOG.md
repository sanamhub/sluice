# Changelog

All notable changes to Sluice are in this file. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0](https://github.com/sanamhub/sluice/releases/tag/v0.1.0) - 2026-10-29

The first release. Sluice sends files and folders straight from one computer to another over QUIC,
checks every block before it is written, and resumes a transfer that dropped.

### Added

- `sluice send` and `sluice recv` for one file, a folder, or standard input. A folder arrives inside
  a folder of its own name, and small files travel together in packs.
- Every block is checked against its BLAKE3 root before it is written. Nothing unverified reaches
  the output folder.
- A receive that was killed or disconnected resumes from the ranges it already holds, and a sender
  that restarts keeps its ticket.
- `sluice send --live` follows a file that is still being written, and the receiver reads each
  segment as it is sealed.
- `sluice inspect` prints what a ticket offers without fetching any content. `--include` takes part
  of a folder, on `inspect` and `recv`.
- `--range` fetches one byte range of a file, and `--out -` writes to standard output.
- Several tickets can be received through one queue, with `--priority` and a retry rule for dropped
  connections.
- `--limit-rate` caps either side. Size flags read `K`, `M`, `G` in powers of 1024 and `KB`, `MB`,
  `GB` in powers of 1000.
- `sluice send --keep N` serves one ticket to N receivers and names each one as it connects.
- `sluice doctor` checks the home folder, its permissions and, with `--network`, the network path.
- `sluice id` prints and rotates the endpoint id, and `sluice completions` writes shell completions.
- `--json` on every command, with exit codes and output shapes that scripts can rely on.
- Binaries for Linux, macOS and Windows on x86_64 and arm64, shell and PowerShell installers, a
  CycloneDX SBOM, and GitHub artifact attestations for every file.
- Two images on ghcr.io: `sluice`, the command-line tool, and `sluice-relay`, a relay you run
  yourself. The relay ships closed, with an empty allowlist.

### Security

- Paths in a manifest are checked before anything is created: no absolute path, no `..`, and no
  link followed outside the output folder.
- Names a sender chose are escaped on every line a terminal prints.
- The home folder and its key are refused when another user owns them or can read them.
- A sender serves nothing until its mode is set, so a ticket cannot be answered before it is
  printed.
- A ticket is a read capability. The
  [threat model](https://github.com/sanamhub/sluice/blob/main/docs/threat-model.md) says what is
  protected and what is not.
