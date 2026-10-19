# Threat model

What Sluice protects against, what it does not, and the design choice behind each. To report a
vulnerability, see [`SECURITY.md`](../SECURITY.md).

## Assets

- **File content**, in transit and in the receiver's partial files.
- **Tickets.** A ticket names content and lets anyone holding it read that content. It is a
  capability, not an address.
- **The identity key**, `$SLUICE_HOME/identity.key`. Whoever holds it can act as that endpoint.
- **Relay bandwidth**, which costs the operator money.

## Protected

| Threat | Defense |
|---|---|
| A relay reads or alters traffic | QUIC with TLS 1.3 end to end. Every 16 KiB chunk group is verified against the BLAKE3 root before any caller sees it |
| A provider serves bad data in a multi-provider fetch | The same verification. Providers that fail it are scored down and dropped |
| Dialing reaches the wrong peer | Endpoints are dialed by Ed25519 public key, not by address |
| Recording traffic now to decrypt later with a quantum computer | Hybrid `X25519MLKEM768` key exchange (RFC 10024) on native builds. The `aws-lc-rs` provider is passed in explicitly, and a connection that negotiated any other group is closed, because a build or feature change can downgrade it with no error. ADR 0018 |
| A malicious manifest writes outside the output folder | The receiver rejects absolute paths, `..`, empty components, NTFS alternate data streams, Windows reserved names and symlinks that escape the root. Fuzzed |
| A ticket holder fills the sender's disk | `iroh-blobs` push is disabled, and the `get`, `get_many` and `observe` modes are set together to the ticket's allowlist. Restricting `get` alone would leave the other two open. Tested |
| A leaked relay URL is used as free bandwidth | Relays run with authorization on. `open` mode needs a flag and has a per-client byte rate limit. ADR 0011 |
| Short-code guessing | Short codes use SPAKE2 and are rate-limited per code at the rendezvous |
| Session secrets in logs | `rustls` 0.23.46 or later, and no log field formats a TLS connection, session or handshake object at any level |
| An agent tricked into a transfer through MCP | Tool descriptions are fixed in the binary. Peer-supplied strings (paths, link targets, relay URLs, error text) come back in structured fields marked untrusted. Accepting content from a peer, pairing, widening relay mode and rotating identity all need confirmation or a policy allowlist |

## Not protected

- **Forged identity by a future quantum adversary.** Identity is Ed25519. Key exchange is post-quantum;
  signatures are not.
- **The browser build's key exchange.** `aws-lc-rs` does not build for WASM, so the browser uses `ring`
  and classical X25519. The session panel shows which group was negotiated.
- **A compromised endpoint.** A stolen identity key is a stolen identity.
- **Traffic analysis.** A relay sees that two endpoints exchanged some volume at some time. Browser
  sessions are always relayed, so this applies to every one of them.
- **Endpoint ids staying private.** Relays and discovery services see them. Nothing in Sluice depends
  on an endpoint id being secret; the content root and the allowlist protect the content.
- **Anyone you gave a ticket to.** There is no revocation beyond expiry and one-time use.
- **Connection floods against an `open` relay.** `iroh-relay` has no working connection-accept limit,
  and until we build one, `open` mode is limited by bytes only.

## Known weaknesses

- **`spake2` has no independent audit**, by its own README. It is used only for short codes, which are
  optional, off by default and rate-limited. Use full tickets when that matters.
- **Rotating the identity key** breaks every pairing and every unexpired ticket naming the old id.

## Supply chain

- Releases are built in CI with GitHub artifact attestations: SLSA provenance and an SBOM in a public
  transparency log. Verify with `gh attestation verify` or `cosign`.
- Binaries embed their dependency list through `cargo-auditable`.
- `cargo-deny` fails the build on known advisories and unclear licenses.
- Windows binaries are signed and macOS binaries notarized.
- Installer scripts verify a signature before running anything.
