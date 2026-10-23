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
| A malicious manifest makes two entries one file on a case-insensitive disk | Paths that are equal after full Unicode case folding and NFC are rejected on every platform, so `README.md` and `Readme.md` never both arrive. Fuzzed |
| A malicious manifest exhausts the receiver's memory | The byte length and the entry count are checked against limits (256 MiB and 1,000,000 by default) before anything is read or reserved. Fuzzed |
| A malicious manifest plants a setuid binary | `mode` holds permission bits only, and a receiver never applies the setuid or setgid bit, whatever the manifest says |
| A provider serves a forged or reordered live stream | Every live record is signed under the ticket's key, with a label and the stream id in the signed message, and verified strictly. A record that does not verify, or comes too early, is refused and another provider can give it. Only two validly signed histories, or a signed record that breaks the chain, abort the stream. Fuzzed |
| A ticket holder fills the sender's disk | `iroh-blobs` push is disabled, and the `get`, `get_many` and `observe` modes are set together to the ticket's allowlist. Restricting `get` alone would leave the other two open. Tested |
| A leaked relay URL is used as free bandwidth | Relays run with authorization on. `open` mode needs a flag and has a per-client byte rate limit. ADR 0011, enforced by the guard below |
| An operator's relay config that opens more than it reads as: `access` left out or misspelled, any key iroh-relay would ignore, a missing config file, HTTPS turned off with `dangerous_http_only`, metrics left on by default with no login, or a secret in the HTTP access URL, which iroh-relay logs | Both images start through `sluice-relay-guard`, which refuses each of these with exit code 64 and one line. The line prints no value, and names a key only if it is 1 to 32 lower-case letters or `_`, and an argument only if it is `--` and 1 to 32 lower-case letters or `-`, so a token pasted in the wrong place stays out of the log. An open relay starts only with `--open` and a per-client byte limit. ADR 0019 and ADR 0020, [docker/guard](../docker/guard/README.md). Tested in each image on every build, fuzzed, and fuzzed against a mirror of iroh-relay's own reading |
| The relay config is replaced between the guard's check and the relay's start, as a Kubernetes ConfigMap update does | The guard hands iroh-relay the open file it checked, as standard input, and iroh-relay reads `/proc/self/fd/0`. A rename of the path cannot reach it. Before it starts iroh-relay, the guard checks that `/proc/self/fd/<n>` names the same device and inode as the open file, and exits 69 and starts nothing if not, because iroh-relay runs open when its config path does not resolve, as in a runtime without `/proc`. ADR 0020 |
| A relay behind a proxy, with no `[tls]` in its config | Allowed, since the proxy ends TLS. Tokens then cross the hop from the proxy to the relay in clear text, so that hop must stay on one host or a private network. The relay README says so |
| Another local user reaches the identity key through the home folder, or plants a link to turn the store's files on another file | A new or empty home is made private: mode 0700 on Unix, read back because some mounts accept `chmod` and change nothing, and a private access list on Windows. A home that another user owns is refused, since its owner can undo that. `lock` is opened without following a link, and `lock.pid` is replaced and not followed. Every file call uses the path the system resolved, in its verbatim form on Windows, and not the one that was typed. `sluice doctor` checks the owner, the mode or list, and the key. Tested on both platforms, except a symbolic link on a Windows user without the right to make one, where a junction stands in |
| A rendezvous server becomes an open relay when its `enable_relay = false` line is deleted | The rendezvous image starts through the same guard with `--rendezvous`, which requires that line, and refuses `access` and `[limits]`. ADR 0020 |
| Short-code guessing | Short codes use SPAKE2 and are rate-limited per code at the rendezvous |
| Session secrets in logs | `rustls` 0.23.46 or later, and no log field formats a TLS connection, session or handshake object at any level |
| An agent tricked into a transfer through MCP | Tool descriptions are fixed in the binary. Peer-supplied strings (paths, link targets, relay URLs, error text) come back in structured fields marked untrusted. Accepting content from a peer, pairing, widening relay mode and rotating identity all need confirmation or a policy allowlist |

## Not protected

- **Forged identity by a future quantum adversary.** Identity is Ed25519. Key exchange is
  post-quantum; signatures are not.
- **The browser build's key exchange.** `aws-lc-rs` does not build for WASM, so the browser uses
  `ring` and classical X25519. The session panel shows which group was negotiated.
- **A compromised endpoint.** A stolen identity key is a stolen identity.
- **Traffic analysis.** A relay sees that two endpoints exchanged some volume at some time. Browser
  sessions are always relayed, so this applies to every one of them.
- **Endpoint ids staying private.** Relays and discovery services see them. Nothing in Sluice
  depends on an endpoint id being secret; the content root and the allowlist protect the content.
- **Anyone you gave a ticket to.** There is no revocation beyond expiry and one-time use.
- **Connection floods against an `open` relay.** `iroh-relay` has no working connection-accept
  limit, and until we build one, `open` mode is limited by bytes only.
- **An operator who skips the guard.** `docker run --entrypoint /usr/local/bin/iroh-relay` runs the
  server on whatever it is given, open default included. Environment variables such as
  `IROH_RELAY_ACCESS_TOKEN` also change what the checked file says, past the guard; the image
  READMEs list them.
- **An edit in place of the relay config** in the instant between the guard's read and the relay's.
  Standard input pins the file, not its bytes. ADR 0020.

## Known weaknesses

- **`spake2` has no independent audit**, by its own README. It is used only for short codes, which
  are optional, off by default and rate-limited. Use full tickets when that matters.
- **Rotating the identity key** breaks every pairing and every unexpired ticket naming the old id.
- **A folder swapped for a link while a sender reads it.** The sender opens each file without
  following a link, but it still follows a folder on the way to that file. Another local user who
  can write into the folder being sent can swap a subfolder for a link between the listing and the
  read, and make the sender send a file its own user can read. Send folders that only you can write
  to. Closing this takes opening each folder by handle.

## Supply chain

A release is built by `release.yml` from a tag, after every check of `ci.yml` passes, and published
only after a second person approves it, which holds only while the `release` environment does not
let administrators bypass its rules. That is a repository setting, which RELEASING.md names and
which GitHub leaves on by default. A failed build publishes nothing, but an image push that fails
after the GitHub release leaves the release without its images until `image-publish` is run again.
[RELEASING.md](../RELEASING.md) lists every file and says how to check one. What a release has:

- **Build provenance attestations,** signed through Sigstore into its public transparency log, for
  every file of the GitHub release, for each image's index, and for each image's amd64 and arm64
  manifest. Each image architecture also has an SPDX SBOM attestation, and the binaries' CycloneDX
  SBOM is a file of the release. `gh attestation verify` checks one, and only with
  `--signer-workflow` and `--source-ref` does it say which workflow and which tag built it.
- **Checksums.** Each archive and the source have a `.sha256` file beside them, and `sha256.sum`
  lists every archive, installer and the source.
- **What the installers check.** Read from the two that `dist` 0.33.1 generates, built in WSL on
  2026-10-23 the way `release.yml` builds them:
  - `sluice-cli-installer.sh` downloads the archive for the machine from the GitHub release with
    `curl -sSfL` or `wget`, and compares its SHA-256 with the value `dist` wrote into the script
    when it built it. A mismatch stops the install. A machine without `sha256sum` gets the line
    `skipping sha256 checksum verification` and the install goes on unchecked.
  - `sluice-cli-installer.ps1` checks nothing. It downloads the zip and unpacks it.
  - Neither checks a signature or an attestation, and both take another download address from
    the environment (`SLUICE_CLI_DOWNLOAD_URL` and three more). The checksum in the shell script is
    only as good as the copy of the script that was run.
- **Each binary carries its dependency list,** through `cargo-auditable`, which `cargo audit bin`
  checks against the RustSec database.
- **`cargo-deny`** fails the build on known advisories and unclear licenses, for the workspace and
  for the guard.

### Not done yet

- **The binaries are not signed.** There is no Authenticode signature on Windows and no
  notarization on macOS, so SmartScreen and Gatekeeper warn on the first run. Both need accounts
  the maintainer does not have yet: Azure Artifact Signing, and an Apple Developer ID. RELEASING.md
  says the same under **Not done yet**.
- **The installers verify no signature,** and the PowerShell one no checksum either. Until they do,
  the way to check a download is `gh attestation verify` on the archive, as RELEASING.md shows,
  before running anything from it.
