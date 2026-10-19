# Roadmap

What gets built, in what order, and what does not get built at all. The phases are ordered so the
riskiest assumption is tested before anything depends on it. Dates are deliberately absent; each phase
ends when its exit criteria pass.

## M0: measure the transport

`noq` has no published throughput numbers, and two open iroh issues report it well below link speed:
[#4286](https://github.com/n0-computer/iroh/issues/4286) (about 40% of a LAN link with BBR, far worse
with Cubic, on macOS) and [#1943](https://github.com/n0-computer/iroh/issues/1943) (about 300 Mbit/s
where netcat reached 700). Before any product code, M0 measures on Linux 10 GbE and on macOS:

- `iroh` plus `iroh-blobs` with one connection and with N connections
- each with default and with tuned windows, and with Cubic and BBRv3
- against `iperf3`, single and parallel, and a parallel-TCP tool (rclone or mscp) as the bar to beat
- on S1, S3 and S6 from [`performance.md`](performance.md#scenarios), with the one-file workload. The
  small-file and mixed-tree workloads need the engine, so they start in 1.0

It also measures:

- the direct-connection rate with only a rendezvous server and no relay (ADR 0013)
- BLAKE3 WASM throughput in Chrome, Firefox and Safari, desktop and iOS (ADR 0014)
- the negotiated key exchange group, asserted to be `X25519MLKEM768` (ADR 0018)

The repository gets its skeleton in the same phase: the three 1.0 crates, CI with every gate from
[`design.md`](design.md#tooling), and the bench harness.

**Exit:** the numbers are published, and they settle one question. Either 1.0 ships QUIC only, or
1.0 also ships the TCP bulk path (ADR 0016). If even that cannot reach the 10 GbE target, the target
is revised in [`performance.md`](performance.md) with the measurement attached.

## 1.0: the engine and the CLI

This is the release that makes the repository public.

- `sluice send` and `sluice recv` for files, folders and pipes
- resume after any interruption
- verified byte ranges (`--range 4G:4.1G`)
- live mode for files still being written
- directory manifest and small-file packing
- the transfer queue with priorities and bandwidth caps
- direct or relayed shown on every transfer, and `sluice doctor`
- the `rendezvous` and `relay` container images
- signed releases for Linux, macOS and Windows through `dist`
- `SPEC.md` with test vectors and a conformance runner
- the benchmark page with S1 to S9 results

**Exit:** the [`performance.md`](performance.md) targets for LAN, WAN, small files and resume are met
or revised in public. A 100 GB transfer completes between two NATs with only a self-hosted rendezvous.
Every at-rest budget is enforced in CI.

## 1.1: programmable

- The control API (`proto/sluice/v1`) and `sluice mcp`.
- In-process bindings for Python, Node, Swift and Kotlin, and the C header.
- Delta transfers.
- Mount on Linux and Windows, and on macOS through FSKit if it is ready.
- Short codes, QR codes, paired devices, signed receipts.
- Fan-out to several receivers, and fetching from several providers at once.
- The TCP bulk path, if M0 left it out of 1.0.

**Exit:**

- An agent moves a 40 GB dataset over MCP with no shell access.
- A delta resend of 50 GB with 1% changed moves under 3% of the bytes.

## 1.2: reach

- The browser build and the Svelte PWA.
- The Tauri desktop app on the same UI.
- Go and C# bindings, and generated clients for the remaining languages.
- A Helm chart for the relay.

**Exit:** a 20 GB transfer from Chrome, Firefox and Safari to a native peer through a self-hosted
relay, published with each browser's ceiling and the relay's specification.

## Later, if measurements ask for it

- io_uring through `compio`, if a profile shows disk in the path.
- Forward error correction (RaptorQ), if measured loss justifies it.
- Mobile apps through Tauri, stating iOS background limits up front.
- Parking a transfer for when both sides are offline.
- Post-quantum signatures (ML-DSA) once the ecosystem supports them.
- A multi-user control service with policy and audit, as its own proposal, once there is demand for it.

## Not built

These are solved elsewhere, or would turn Sluice into a different product. A pull request adding one
needs a discussion first.

- A QUIC stack, NAT traversal, a relay protocol, or a content-addressed store. iroh has them.
- Folder sync. Syncthing does it well.
- A hosted service, accounts, or any telemetry.
- Any API that carries file bytes.
- A second UI codebase.

## Open questions

1. The `noq` throughput numbers, M0.
2. Direct-connection rate with a rendezvous only, M0.
3. Live segment size and pack size. 8 MiB and 64 MiB are unmeasured. Both must be multiples of the
   16 KiB block, and they are (512 and 4096 blocks), so keeping them until measured costs nothing.
4. The 64 KiB chunk group rule in `SPEC.md` section 5. It would cut outboard overhead from about 0.4%
   to 0.1% for blobs over 8 GiB, but `IROH_BLOCK_SIZE` is a constant in `iroh-blobs`, so there is no
   seam for it while static content goes through that crate. The options are:
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
| `noq` cannot reach the targets | M0 first; then upstream patches (ACK frequency is the likely one); then the TCP bulk path |
| `iroh-blobs` changes its protocol before 1.0 | Pinned versions, our ticket envelope, the interop matrix, spec vectors |
| Six dependencies from one company | Permissive licenses, a stable wire, small crates vendorable in days |
| A dependency changes a default under us | Access modes and the crypto provider are set explicitly and asserted in tests |
| Resume corrupts a file | The barrier model, `turmoil` torn-write tests, fuzzing, and hand review of the resume code |
| Small-file workloads look slow | Packing, with its own target |
| Operators expect the internet path to need nothing | The connectivity table is in the README, and `doctor` explains the user's case |
| The browser is unusable without a relay | The web UI names its relay on first run, and the relay is one `docker run` in the quickstart |
