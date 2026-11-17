# 0040. A tab is a peer with the same post-quantum handshake, through one relay the website names

**Status:** Proposed **Date:** 2026-11-17 **Relates to:** ADR 0011, ADR 0014, ADR 0018, ADR 0036
**Supersedes:** ADR 0018's point 5

## Context

ADR 0018 closes every connection that does not negotiate `X25519MLKEM768`. The browser build uses
`ring`, which has no ML-KEM, so a tab could not pass that check, and ADR 0036 made the page a remote
control for a local daemon instead of a peer. ADR 0018 point 5 left open how a native peer would
accept a classical handshake from a browser, and only from one.

A spike settled the question the other way. The browser build's `ring` provider gains one key
exchange group: `ring`'s X25519 and RustCrypto's `ml-kem` 0.3.4, combined as rustls combines them
for `X25519MLKEM768`, the post-quantum share first, 1184 + 32 bytes from the client and 1088 + 32
from the server. Against an unmodified `aws-lc-rs` peer, both ends reported `X25519MLKEM768` in Node
26, Chrome 157 and Firefox 159. Through a relay on the same machine:

| | Handshake, median of 50 | 256 MiB, median of 5 |
|---|---|---|
| Chrome, `X25519` | 14.0 ms | 19.0 MiB/s |
| Chrome, `X25519MLKEM768` | 15.8 ms | 19.0 MiB/s |
| Firefox, `X25519` | 9.0 ms | 16.8 MiB/s |
| Firefox, `X25519MLKEM768` | 13.0 ms | 16.9 MiB/s |

The module grows by 145 182 bytes raw and 22 059 bytes with brotli, 2.3% and 1.8%. The cost is in
the handshake, once per connection; the bulk cipher is the same either way.

A tab cannot open a UDP socket, so it reaches every peer through a relay for the whole transfer (ADR
0011). A person who opens the website has no relay of their own.

## Decision

1. **The browser build negotiates `X25519MLKEM768` and nothing else.** It offers only the hybrid
   group, reads the negotiated group after the handshake as the native build does, and closes the
   connection with the same `Policy` error otherwise. No peer anywhere accepts a classical
   handshake, from a browser or not. ADR 0018 point 5 is superseded: there is no second profile.
2. **A tab is an ordinary peer.** Its endpoint id is authenticated at the handshake, the content it
   receives is verified against the ticket's root, and the rules of `SPEC.md` apply to it unchanged.
   Its key is made when the page loads and kept in memory only, never in browser storage, so a tab
   holds no grant of ADR 0039 and cannot be paired.
3. **The website names one relay, run by the project.** It is the relay image of ADR 0028 with the
   guard of ADR 0019, on a host that costs nothing, in `open` mode with a per-client rate and a
   per-transfer size limit. The website shows which relay it uses on its first screen and in each
   transfer, as ADR 0011 point 5 asks, and a setting points it at any other relay. A native peer
   keeps its own relay settings: a ticket names the sender's relay, and a tab dials it.
4. **A tab writes what it receives to disk as it arrives.** Holding a whole blob in memory, as the
   spike's `fetch_blob` does, is refused for the product. Chromium writes through the File System
   Access API; Firefox and Safari stream the download through a service worker. The website says
   when a file is large enough that the desktop app is faster, and why.
5. **CI proves the handshake in two browsers.** A job opens the page in Chromium and Firefox against
   a native peer and fails unless both sides report `X25519MLKEM768`.

## Alternatives

- **A classical handshake for browser peers only**, as ADR 0018 point 5 anticipated. Refused: the
  spike shows the hybrid group costs 2 to 4 ms per connection and no throughput, so there is nothing
  to trade the guarantee for.
- **`rustls-rustcrypto`** as the whole provider. Refused: its 0.1.0-pre.0 has no ML-KEM and is a
  pre-release.
- **`rustls-post-quantum`.** Refused: every release depends on `aws-lc-rs`, which has no build for
  `wasm32-unknown-unknown`.
- **n0's public relays for the website.** Refused: n0 documents them for development and hobby use,
  with shared rate limits and no service level, and ADR 0011 does not inherit a relay the project
  does not run.
- **The page as a remote control only**, as ADR 0036 stands. Kept for a computer that has the app or
  the daemon, and no longer the only way: a visitor with nothing installed needs a peer.

## Consequences

- The browser build carries seven RustCrypto crates (`ml-kem`, `module-lattice`, `kem`, `sha3`,
  `keccak`, `shake`, `sponge-cursor`), Apache-2.0 or MIT. None has an independent audit; `ml-kem`
  carries the FIPS 203 known-answer tests. The native build keeps `aws-lc-rs`. The threat model
  names the difference.
- rustls's own combiner is not public, so the browser build writes the order and the sizes out, and
  a host test runs it against `aws-lc-rs` on every CI run.
- The project runs one relay. Its egress is the cost of the website, its caps are what keep it free,
  and it stores nothing and logs no transfer. The roadmap's "no hosted service" stays true in that
  sense and says so.
- A transfer from a tab is as fast as that relay allows, never faster. The website says so, and the
  desktop app is the answer for large files.
- ADR 0036's page is unchanged. A visitor whose computer runs the app or the daemon is offered it.
