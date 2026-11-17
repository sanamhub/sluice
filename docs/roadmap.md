# Roadmap

What 1.0 includes, what comes after, and what Sluice does not build. Dates are deliberately absent:
a release ships when its exit criteria pass.

## Who Sluice is for

Anyone who needs to move a file, with three ways in that all reach the same engine:

| Who | Way in | What they need to know |
|---|---|---|
| Someone who has never opened a terminal | The website, or the desktop app | Where the file is, and who gets it |
| An engineer | The CLI, the container image, the bindings | The flags, or the API |
| An AI agent | `sluice mcp` and the control API | The tool list |

No way in is the main one. The README, the website and the docs lead with sending a file and offer
all three, and the app and the website need no command to start.

## Before 1.0: measure the transport

`noq` has no published throughput numbers, and two open iroh issues report it well below link speed:
[#4286](https://github.com/n0-computer/iroh/issues/4286) (about 40% of a LAN link with BBR, far
worse with Cubic, on macOS) and [#1943](https://github.com/n0-computer/iroh/issues/1943) (about 300
Mbit/s where netcat reached 700). The transport is measured on Linux 10 GbE and on macOS before
anything depends on it:

- `iroh` plus `iroh-blobs` with one connection and with N connections
- each with default and with tuned windows, and with Cubic and BBRv3
- against `iperf3`, single and parallel, and a parallel-TCP tool (rclone or mscp) as the bar to beat
- on S1, S3 and S6 from [`performance.md`](performance.md#scenarios), with the one-file workload.
  The small-file and mixed-tree workloads need the engine, so they start in 1.0

It also measures:

- the direct-connection rate through a meeting-point relay, and that its 64 KiB/s limit leaves hole
  punching working (ADR 0028)
- BLAKE3 WASM throughput in Chrome, Firefox and Safari, desktop and iOS (ADR 0014)
- the negotiated key exchange group, asserted to be `X25519MLKEM768` (ADR 0018)

The bench harness in `bench/` runs all of it.

**Exit:** the numbers are published, and they settle one question. Either 1.0 ships QUIC only, or
1.0 also ships the TCP bulk path (ADR 0016). If even that cannot reach the 10 GbE target, the target
is revised in [`performance.md`](performance.md) with the measurement attached.

## 1.0: the engine and the CLI

- `sluice send` and `sluice recv` for files, folders and pipes
- resume after any interruption
- verified byte ranges (`--range 4G:4.1G`)
- live mode for files still being written
- directory manifest and small-file packing
- the transfer queue with priorities and bandwidth caps
- the ticket as a QR code on a terminal, with `send --qr`
- direct or relayed shown on every transfer, and `sluice doctor`
- the `relay` container image, with the meeting-point and full-relay recipes
- signed releases for Linux, macOS and Windows through `dist`
- `SPEC.md` with test vectors and a conformance runner
- the benchmark page with S1 to S9 results

**Exit:** the [`performance.md`](performance.md) targets for LAN, WAN, small files and resume are
met or revised in public. A 100 GB transfer completes between two NATs that can hole punch, through
a self-hosted meeting point that carries none of the file. Every at-rest budget is enforced in CI.

## 1.1: programmable

- The control API (`proto/sluice/v1`) and `sluice mcp`.
- In-process bindings for Python, Node, Swift and Kotlin, and the C header.
- Delta transfers.
- Mount on Linux and Windows, and on macOS through FSKit if it is ready.
- Short codes, and signed receipts.
- Paired devices: a daemon keeps the endpoint ids it trusts, each with a `deliver` or `control`
  grant, so one computer sends into another's inbox without a ticket, and a second computer drives
  the daemon. [ADR 0039](adr/0039-paired-devices-and-remote-control.md)
- Fan-out to several receivers, and fetching from several providers at once.
- The TCP bulk path, if the transport measurement left it out of 1.0.

**Exit:**

- An agent moves a 40 GB dataset over MCP with no shell access.
- A delta resend of 50 GB with 1% changed moves under 3% of the bytes.

## Alongside 1.1: the front door

Work that needs no new engine feature, done while 1.1 is built:

- The README and the website start with sending a file, show the three ways in side by side, and
  put the CLI under "For developers". They say plainly that the app and the website send are coming,
  and link the page a person can use today.
- A page for AI agents: how to add `sluice mcp` to an assistant, with the tool list, and an
  `llms.txt` on the website.
- A post-quantum handshake in the browser build. The browser build uses `ring`, which has no ML-KEM,
  so a tab cannot pass the check of ADR 0018 and cannot be a peer. A spike builds `X25519MLKEM768`
  for `wasm32` from RustCrypto's `ml-kem` and `ring`'s `X25519`, and measures the handshake time,
  the `.wasm` size and the transfer rate against the classical build. Its result decides whether the
  website sends in 1.2 (below).

## 1.2: for everyone

- **The desktop app.** Tauri, on the Svelte UI of ADR 0009 that the daemon's page already uses, with
  the engine in the same process. Opening the app is the whole setup: no terminal, no `sluice ui`, no
  separate daemon to start. It stays in the tray to receive from paired devices (ADR 0039), and it
  can add `sluice mcp` to a local AI assistant with one button.
- **Installers that a beginner can open.** No signing certificate is bought:
  - Windows: the Microsoft Store, where the store signs the package, and `winget`.
  - macOS: a `.dmg` signed ad hoc and not notarized, with the "Open Anyway" steps on the download
    page. The website is the way to send from a Mac with nothing installed.
  - Linux: Flathub, an AppImage and a `.deb`.
- **Sending and receiving in a tab, if the spike above passes.** The tab is a peer with the same
  post-quantum handshake as the native build, so no transfer is weaker for involving a browser. A tab
  cannot connect directly (ADR 0011), so every byte goes through a relay: the website uses one relay
  that the project runs on a free host, with a per-visitor rate and size cap, and anyone can point it
  at their own. The website says when a file is large enough that the app is faster. If the spike
  fails, the website drives a local app or daemon and asks a visitor without one to install the app.
- **The PWA.** The same UI installed from the website, for phones until the mobile apps exist.
- **A first run that explains itself.** Pick or drop files, get a link and a QR code, see "direct"
  or "relayed" and why, with no word a beginner has to look up. The link opens the website with the
  ticket in its fragment, which a browser never sends to a server, and the website offers to open
  the app when one is installed.
- **Accessibility.** The UI meets WCAG 2.2 AA, checked with a screen reader on each platform.

**Exit:**

- Three people who have never used a terminal each install the app from its store or download page
  and send a 5 GB folder from Windows to macOS, with no help and no command. Where they get stuck is
  published, and fixed.
- A 1 GB transfer from Chrome, Firefox and Safari, with nothing installed, to a desktop app, through
  the website's relay, published with each browser's rate.
- A 20 GB transfer from Chrome, Firefox and Safari to a native peer through a self-hosted relay,
  published with each browser's ceiling and the relay's specification.

## 1.3: reach

- Go and C# bindings, and generated clients for the remaining languages.
- A Helm chart for the relay.

## Later, if measurements ask for it

- io_uring through `compio`, if a profile shows disk in the path.
- Forward error correction (RaptorQ), if measured loss justifies it.
- Mobile apps through Tauri, stating iOS background limits up front.
- A notarized macOS build, when someone sponsors the Apple developer fee.
- Parking a transfer for when both sides are offline.
- Post-quantum signatures (ML-DSA) once the ecosystem supports them.
- A multi-user control service with policy and audit, as its own proposal, once there is demand for
  it.

## Not built

These are solved elsewhere, or would turn Sluice into a different product. A pull request adding one
needs a discussion first.

- A QUIC stack, NAT traversal, a relay protocol, or a content-addressed store. iroh has them.
- Folder sync. Syncthing does it well.
- A hosted service, accounts, or any telemetry. The one relay the website uses carries encrypted
  bytes it cannot read, stores nothing, and keeps no record of who sent what.
- Any API that carries file bytes.
- A second UI codebase.

## Open questions

1. The `noq` throughput numbers, from the transport measurement.
2. The direct-connection rate through a meeting point, from the same measurement.
3. Live segment size and pack size. 8 MiB and 64 MiB are unmeasured. Both must be multiples of the
   16 KiB block, and they are (512 and 4096 blocks), so keeping them until measured costs nothing.
4. The 64 KiB chunk group rule in `SPEC.md` section 5. It would cut outboard overhead from about
   0.4% to 0.1% for blobs over 8 GiB, but `IROH_BLOCK_SIZE` is a constant in `iroh-blobs`, so there
   is no seam for it while static content goes through that crate. The options are:
   - make the block size a store parameter upstream
   - fork `iroh-blobs`
   - bypass it for large blobs
   - drop the rule and accept 0.4%, which is about 800 MB on a 200 GB file

It has to be settled before the spec gets test vectors for it.
5. Three `SPEC.md` rules that still need writing, each with test vectors:
   - push disabled, and the `get`, `get_many` and `observe` access modes set together
   - the order ALPNs are registered in, which iroh negotiates by
   - what a receiver does with two validly signed live frames for the same `seq`

## Risks

| Risk | Mitigation |
|---|---|
| `noq` cannot reach the targets | Measure the transport first; then upstream patches (ACK frequency is the likely one); then the TCP bulk path |
| `iroh-blobs` changes its protocol before 1.0 | Pinned versions, our ticket envelope, the interop matrix, spec vectors |
| Six dependencies from one company | Permissive licenses, a stable wire, small crates vendorable in days |
| A dependency changes a default under us | Access modes and the crypto provider are set explicitly and asserted in tests |
| Resume corrupts a file | The barrier model, `turmoil` torn-write tests, fuzzing, and hand review of the resume code |
| Small-file workloads look slow | Packing, with its own target |
| Operators expect the internet path to need nothing | The connectivity table is in the README, and `doctor` explains the user's case |
| The browser is unusable without a relay | The website uses the project's relay by default and names it, and a self-hosted relay is one `docker run` in the quickstart |
| The website's relay is abused or outgrows its free host | A per-visitor rate and size cap through the guard (ADR 0019), the app named as the way for large files, and the relay replaceable in one setting |
| A beginner meets an operating system warning on install | The Microsoft Store signs the Windows package; the macOS download page shows the "Open Anyway" steps; the website sends with nothing installed |
| The browser cannot do the post-quantum handshake | The spike decides it before 1.2 work starts; if it fails, the website drives a local app instead of being a peer, and no classical handshake is accepted |
