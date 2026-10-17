# Competitors, community, and upstream drift, checked 2026-10-17

Evidence for the competitor, prior-art, upstream-version, and community claims in `plan.md`. Each claim
links its source. Everything here was read or searched on 2026-10-17.

Treat community findings as directional. Search reached few Reddit or X threads, so most of that
evidence is from Hacker News, Lemmy, and press. Section 7 lists claims that were not verified.

---

## 1. Headline

Five findings matter. Two need action now, and three change how the product is pitched.

| # | Finding | Severity | Where it hits |
|---|---|---|---|
| 1 | **`iroh-blobs` 0.94.0 to 0.103.0 accepted unauthenticated push into the provider store.** Fixed in 0.103.1 (2026-10-06), and the broken versions were yanked. `get_many` and `observe` now have their own access modes, so restricting `get` alone leaves both open | **Act now** | plan 2.1, 13; SPEC 4 and 5 |
| 2 | **`rustls` 0.23.46 (2026-10-14) fixes four advisories.** One leaked master secrets and resumption PSKs through `Debug` formatting of sessions | **Act now** | plan 2.1, 13; the AGENTS.md logging rule |
| 3 | **Live transfer of growing content is not new.** Hypercore and Hyperdrive have shipped signed append-only Merkle logs, live tailing, and sparse verified byte ranges for years under permissive licenses. p2panda does signed append-only logs on iroh | Positioning | plan 1, 3 ("Nobody has it"); README |
| 4 | **MCP file transfer is not new either.** `ffl-mcp` (Apache-2.0) already gives agents P2P file transfer over MCP | Positioning | plan 1, 14 |
| 5 | **"Every browser-based transfer tool has the same ceiling" is false.** It holds for iroh, but WebRTC data channels hole-punch from a tab, and FilePizza and PairDrop use them | Correctness | plan 10.1, README |

What still holds up:

- No iroh-stack project publishes throughput numbers, so a benchmark page is still a real gap.
- No transfer tool offers Apache-2.0 embedding together with an API.
- Live verified tailing of an ordinary file, on iroh and BLAKE3, inside a ticket-style transfer tool, is
  still unclaimed.
- `mount` and `doctor` are still unclaimed.

---

## 2. Upstream versions

| Item | Earlier pin | Current | Action | Source |
|---|---|---|---|---|
| `iroh-blobs` | 0.103.0 | **0.103.1**, a security fix. Still pre-1.0, with no 1.0 or configurable block size announced | Pin `>=0.103.1`. Turn push off explicitly. Set the `get`, `get_many` and `observe` modes together wherever access is restricted. Add a test that a push and an off-allowlist `get_many` are both refused | [iroh-blobs push fix](https://iroh.computer/blog/iroh-blobs-push-fix) |
| `iroh`, `iroh-relay` | 1.2.0 | **1.3.0** (2026-09-28): batch datagram APIs, a relay send-queue stall fix, and the `Router` now negotiating ALPNs **in registration order**, not alphabetical | Register `sluice/1` and the `iroh-blobs` ALPN in a deliberate order, and note it in SPEC section 3 | [iroh 1.3.0](https://iroh.computer/blog/iroh-1-3-0) |
| `noq` | 1.3.0 | Unchanged | None | crates.io |
| `rustls` | 0.23.45 | **0.23.46** | Bump it. Never `Debug`-format TLS session or connection state, even at trace level | [Phoronix](https://phoronix.com/news/Rustls-0.23.46-Released) |
| `dist` | 0.32.0 | **0.33.0** (2026-09-11). The baseline was already stale on the day it was written. Adds Azure Artifact Signing for x86_64 Windows | Update section 9. Consider Windows signing, since unsigned builds were a recurring complaint (section 4) | [dist releases](https://github.com/axodotdev/cargo-dist/releases) |
| Tauri | 2.11 | **2.12.4**: MSRV 1.90, Windows 7 dropped, updater signatures bind the app version. 3.0 is in alpha | Set the workspace MSRV to at least 1.90 before the Tauri crate lands | [Tauri releases](https://github.com/tauri-apps/tauri/releases) |
| `rmcp` | unversioned | **3.5.1** (2026-10-05), releasing about every two weeks | Pin exactly, and add it to the per-quarter churn budget in 11.4 | [rust-sdk releases](https://github.com/modelcontextprotocol/rust-sdk/releases) |
| `bao-tree`, `turmoil`, `fuse3` | 0.16.1, 0.7, 0.9.0 | Unchanged (`turmoil` is 0.7.2) | None | crates.io |
| Relay accept-rate limit | Not implemented | Still not implemented. The per-client byte limit (`client_rx`, `set_client_rate_limit`) is all there is | Plan 6 and 13 stand | [iroh rate limiting](https://docs.iroh.computer/relays/rate-limiting) |

**Endpoint ids are not secret.** iroh's
[blinded address lookup](https://iroh.computer/blog) post (2026-10-13) points out that relays and
lookup services see endpoint ids. SPEC section 4 calls the whole ticket secret, and that is still right,
because the root is what grants read. But nothing should depend on `endpoint_id` staying private, and
the threat model should say so.

**Still not checked:** whether `IROH_BLOCK_SIZE` is still a constant (docs.rs returned 404), and whether
`noq` still defaults to Cubic. The open item from section 0.3 of the plan, the 64 KiB negotiation in
SPEC 5 rule 2, stands until M0 reads the source again.

### Standards

- **Hybrid ML-KEM for TLS is now RFC 10024**, Proposed Standard, August 2026. `X25519MLKEM768` is
  codepoint `0x11EC`. Plan section 13 can cite the RFC instead of the draft.
  [datatracker](https://datatracker.ietf.org/doc/draft-ietf-tls-ecdhe-mlkem/)
- **QUIC Multipath** (`draft-ietf-quic-multipath-21`) is in the RFC Editor queue, so an RFC is near.
  [datatracker](https://datatracker.ietf.org/doc/draft-ietf-quic-multipath/)
- **QUIC NAT traversal** is still an individual draft (`draft-seemann-quic-nat-traversal-02`), not adopted.
- **BBRv3** is still a draft (`draft-ietf-ccwg-bbr-06`), aimed at Experimental status.
- **MASQUE `connect-udp-listen`** (draft -16) is in IESG evaluation and targets P2P and ICE use. It is a
  possible standards-based relay. Watch it, and don't build on it before 1.2.
  [datatracker](https://datatracker.ietf.org/doc/draft-ietf-masque-connect-udp-listen/)

---

## 3. Competitors, 2025-06 to 2026-10

| Tool | License | State | Gap versus Sluice |
|---|---|---|---|
| **DashBeam** (formerly AltSendme) | AGPL-3.0 | About 9.9k stars. Desktop, Android, **CLI and web app**, BLAKE3 chunk resume, self-hosted relays, mDNS, multi-peer send. On iroh 1.0 with multipath | No API, no live mode, no ranges, no mount. The license is now the main structural difference. [repo](https://github.com/tonyantony300/dashbeam) |
| **LocalSend 1.18** (2026-08-10) | Apache-2.0 | Rust networking core, **official CLI**, IPv6, checksums on by default, "receive via link" for browsers | Still LAN-only. CVE-2026-25154 affects 1.17 and earlier. [release notes](https://ubuntuhandbook.org/index.php/2026/08/the-airdrop-alternative-localsend-1-18-0-released-with-cli-support/) |
| **sendme** | Apache/MIT | Packaged in Homebrew, AUR, nixpkgs, FreeBSD. Hit by the push bug, fixed in 0.36.1 | CLI only, no API. [repo](https://github.com/n0-computer/sendme) |
| **Blip** 1.1.x | Closed, paid for commercial use | Linux client added. Strong resume (drops, unplugged drives, full disks) | No source, no API, no benchmarks. [blip.net](https://blip.net) |
| **croc** v10.6 | MIT | Fast release cadence | Still relay-bound |
| **magic-wormhole** 0.24 (2026-05) | MIT | Dilation ships as the `dilate` extra: reconnection and connection hints | Still relay-first. [PyPI](https://pypi.org/project/magic-wormhole/) |
| **Syncthing 2.0** (2025-08) | MPL-2.0 | Database moved to SQLite | Sync, not one-shot send |
| **ffl / ffl-mcp** | Apache-2.0 | P2P over WebRTC with TCP, QUIC, and HTTP relay fallback. MCP tools for sharing, downloading, and session events | **Direct MCP prior art.** No verified ranges, no live mode. [repo](https://github.com/nuwainfo/ffl-mcp) |
| **mcpmesh** | MIT/Apache | Shares MCP servers P2P over iroh 1.0 | Not file transfer, but shows iroh plus MCP is an active niche |
| **iroh-netbench** (2026-08) | MIT/Apache | Measures RTT, jitter, and loaded throughput for iroh | Publishes no results. M0 could use it alongside `noq`'s `perf/` |
| Floe, kiko, AlterSend | various | New small croc-likes in 2026 | Crowded low end |

### Prior art for live and verified-range transfer

| System | What it has | License |
|---|---|---|
| [Hypercore](https://github.com/holepunchto/hypercore) v10 | Signed append-only Merkle log, sparse replication, `createReadStream({ live: true })` | MIT |
| [Hyperdrive](https://github.com/holepunchto/hyperdrive) | Byte-range `createReadStream`, `downloadRange`, `watch()` | Apache-2.0 |
| [p2panda](https://docs.rs/p2panda) | Signed, fork-tolerant, prunable append-only logs on iroh, plus `p2panda-blobs` | MIT/Apache |
| BEP 46 | Only a mutable pointer to a new complete torrent. No append | n/a |
| `iroh-willow` | Unreleased (0.0.1) | n/a |

**What this means.** Sluice's live log (SPEC section 6) is a sound design. In shape it is close to
Hypercore's: signed entries, a hash chain, and blocks that verify independently. Sluice keeps segments
as ordinary BLAKE3 blobs and closes into a static root, which Hypercore does not. Say that plainly
instead of claiming novelty. Before SPEC 0.2, read Hypercore's handling of these four things, because
it has solved them in production:

- fork detection: a sender signing two different entries for the same `seq`
- truncation
- late joiners
- tail provisionality

SPEC section 6 has no rule yet for a signed fork, meaning two valid `Frame`s with the same `seq`. That
is a gap worth a test vector.

---

## 4. What developers say

### Recurring complaints and requests

1. **Show whether a transfer is direct or relayed.** This is the most consistent ask. Commenters praised
   sendme for showing it. [HN, Jun 2025](https://news.ycombinator.com/item?id=44147582)
2. **Resume must never leave a corrupt partial file.** LocalSend gets criticized for exactly that.
   [HN, 2026](https://news.ycombinator.com/item?id=47933208)
3. **Tickets are too long to read out loud.** People want short codes or QR codes. Lemmy readers said
   so about DashBeam. [Lemmy, mid 2026](https://lemmy.4d2.org/post/4524746)
4. **CGNAT and symmetric NAT break P2P for many users.** Some have never gotten a P2P app to work with
   a friend.
5. **One tool for LAN discovery and WAN.**
6. **Signed binaries on all three desktop OSes.** Unsigned macOS builds were a complaint.
7. **Credible mobile and iOS support.** "AirDrop for any device" without an iOS app drew pushback.
8. **Download while the sender is still uploading.** wormhole.app and copyparty users ask for it. This
   is the strongest piece of demand evidence for live mode found in this pass.

### Reception of comparable launches

- **Blip on HN** (Jul 2025): praised for polish and resume. Criticized for being closed source, its
  pricing, relay priority for paying users, and having no benchmarks against Aspera. Several commenters
  called relay fallback a dealbreaker. [HN](https://news.ycombinator.com/item?id=44648772)
- **AltSendme Show HN**: 2 points. Interchangeable P2P senders no longer get attention.
- **iroh 1.0 on HN** (Jun 2026): technical interest, but the value proposition read as unclear ("what is
  this versus Tailscale"). Concerns about relay cost and what happens if n0 disappears.
  [HN](https://news.ycombinator.com/item?id=48542480)

### Attitudes that bear on the plan

- **MCP fatigue is real.** "MCP is dead, long live the CLI" topped HN in Feb 2026, and Thoughtworks put
  thin API-to-MCP wrappers on Hold. Consensus: a good CLI with `--json` beats a thin MCP layer. The
  plan already has `--json` everywhere. Lead with it, and present MCP as one adapter, not a headline.
  [summary](https://mcpplaygroundonline.com/blog/is-mcp-dead-cli-debate)
- **Telemetry is punished.** GitHub CLI's opt-out telemetry in v2.91 drew a 419-point HN thread and a
  nixpkgs patch. The plan's "no telemetry, ever" is a selling point, and the README should keep it
  prominent. [The Register, Apr 2026](https://www.theregister.com/2026/04/22/github_opts_all_cli_users/)
- **Benchmark claims are distrusted.** croc's author leaves speed out on purpose, and loopback vendor
  benchmarks get dismissed. The plan's method (netem profiles, `iperf3` ceiling, raw data, rerun
  script) answers this, provided it also publishes WAN, NAT, and relay-path numbers, not only LAN.
  [croc comparison](https://getcroc.com/blog/compare-file-transfer-tools)
- **"Rust" earns no goodwill on its own.** The Ubuntu 25.10 uutils and sudo-rs backlash made that
  clear. Market outcomes, not the language or iroh. [LWN](https://lwn.net/Articles/1007907/)
- **AI-generated code is a reputational risk.**
  - curl ended its bug bounty in Jan 2026 over AI-slop reports.
  - Ghostty and Godot restricted AI-written contributions.
  - Lemmy called out DashBeam's AI authorship for security-sensitive code.

  Plan 11.3 relies on heavy AI codegen, and AGENTS.md already reserves the three critical areas for
  human hands. Make that visible before any public launch: an AI contribution policy in
  `CONTRIBUTING.md`, human sign-off recorded in CODEOWNERS for the trust boundary, resume, and FFI, and
  fuzzing and property tests visible in CI.
  [BleepingComputer](https://bleepingcomputer.com/news/security/curl-ending-bug-bounty-program-after-flood-of-ai-slop-reports/)
- **Self-hosting the relay is friction for casual users.** Power users like the option, and casual users
  expect relays to just work. Locked decision 1 is a deliberate trade, and the plan already states the
  cost. The research supports keeping it, and also supports making the one-command relay quickstart the
  most polished path in the docs. "No servers" claims get called out. Sluice's "any relay is one you
  run" wording is the honest version.

---

## 5. MCP security

The spec is still 2026-07-28. The registry is still in preview, with its API frozen at v0.1. The live
threats are tool poisoning, meaning instructions hidden in tool descriptions, and prompt injection
through tool output. Coding agents run project-defined MCP servers with the developer's OS privileges.
[CSA note, 2026-07-01](https://labs.cloudsecurityalliance.org/research/csa-research-note-mcp-tool-poisoning-auto-execution-20260701/)

`sluice mcp` returns filenames, manifests, and peer-supplied strings, and all of that is
attacker-controlled. Three rules for `sluice-cli`'s MCP surface, which the plan does not yet state:

1. Tool descriptions are static strings compiled into the binary.
2. Peer-supplied text (paths, symlink targets, relay URLs, error reasons) goes into structured fields
   and is marked untrusted, never interpolated into prose.
3. Anything that makes the machine accept content from or expose content to a peer needs explicit
   confirmation, or a policy allowlist:
   - receiving to a path outside a configured root
   - pairing
   - widening relay mode
   - rotating identity

---

## 6. Other trends

- **macOS mount is now realistic.** FSKit works on macOS 26. fuse-t supports it from 1.2.1, with the
  system toggle in 26.4. macFUSE 5.1 has an opt-in FSKit backend for libfuse3 only. Missing operations
  are byte-range locks, flock, ioctl, and xattrs, none of which a read-only mount needs. Plan 12 lists
  "macOS mount via FSKit" under Later, and it could move to 1.1 if time allows.
  [fuse-t](https://github.com/macos-fuse-t/fuse-t)
- **WinFsp 2026 betas** fix CVE-2026-7162. Require a fixed WinFsp version at mount time.
- **io_uring:** `compio` 0.19 is active and `tokio-uring` is dormant. ADR 0007 stands.
- **EU Cyber Resilience Act.** Reporting obligations for actively exploited vulnerabilities (24-hour
  warning, 72-hour notification, 14-day report) have applied since 2026-09-11. Everything else applies
  from 2027-12-11. Non-commercial open source is out of scope, and open-source stewards have lighter
  duties. Whether Sluice is in scope depends on who distributes it and whether anything is monetized,
  for example a paid hub or support. **This is a legal question. It needs a qualified answer before 1.0
  and should not be assumed either way.**
  [Hogan Lovells](https://www.hoganlovells.com/en/publications/eu-cyber-resilience-act-preparing-for-vulnerability-and-incident-reporting)
  `SECURITY.md` already has a disclosure address, which is the base the CRA builds on.

---

## 7. Not verified

- Whether `IROH_BLOCK_SIZE` is still a constant, and whether `noq` still defaults to Cubic. Both were
  correct on 2026-09-16.
- The exact iroh 1.0 GA date. One source says 2026-06-15, another implies early July.
- DashBeam's star count and v0.7.2 date (secondary sources).
- The WebRTC TURN fallback rate (20 to 30%, one vendor blog).
- How the CRA applies to stewards (secondary sources only).
- Most Reddit and X sentiment. Search reached few threads, and the sections above lean on HN and Lemmy.
