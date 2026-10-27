# Threat model

What Sluice protects against, what it does not, and the design choice behind each. To report a
vulnerability, see [`SECURITY.md`](../SECURITY.md).

## Assets

- **File content**, in transit and in the receiver's partial files.
- **Tickets.** A ticket names content and lets anyone holding it read that content. It is a
  capability, not an address.
- **The identity key**, `$SLUICE_HOME/identity.key`. Whoever holds it can act as that endpoint.
- **Relay bandwidth**, which costs the operator money.

**A ticket on the command line is visible.** A ticket passed as an argument appears in the process
list, where another local user can read it, and the shell writes it to its history. Several tickets
in one `recv` are all there. `sluice recv -` reads one ticket from standard input instead, and a
caller that cares pipes it.

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
| A transfer replaces a file the user did not mean to lose | Every target is checked before the first content request. A top-level name that already exists in the output, as a file, folder or link and checked without following a link, refuses the whole transfer with a `Policy` error naming it; an existing folder counts. A top-level name starting with `.` needs `--allow-hidden`. With `--force`, a clashing file or link is replaced only after its bytes verify and a folder of the same name is written into; a folder is never replaced by a file or a link, even with `--force`, because the rename cannot do it. The final rename cannot replace a name that appeared after the check, and packed files obey `--force` like any other. A resume is a store whose marker names the same ticket, so a clash a writer meets removes the store like any other refusal |
| An existing link in the output redirects a write | A parent that is a symlink is refused before anything is written. A link target is resolved one component at a time the way the operating system does: a component that this transfer makes into a link is replaced by that link's target before the next `..` is applied, with the kernel's limit of 40 links, and a target that leaves the real output is refused. Every link is checked against the complete manifest before any link is created, so the order entries are written in cannot change an answer, and the deepest existing prefix is canonicalized so a target reached through a link already on disk cannot leave either. The same code runs on Windows |
| A sender keeps serving after its ticket expires | The provider checks the expiry on every request, and the serving deadline uses the same clock; at the expiry the router closes every connection, so a get that started before it is cut off |
| A partial get is mistaken for a complete download | The provider counts a download only when the ranges served for a blob add up to the blob's size, so asking for one block leaves the sender serving. A client that fetched from every root and then closed its connection has nothing left to receive, and the sender stops |
| A provider serves a forged or reordered live stream | Every live record is signed under the ticket's key, with a label and the stream id in the signed message, and verified strictly. A record that does not verify, or comes too early, is refused and another provider can give it. Only two validly signed histories, or a signed record that breaks the chain, abort the stream. A receiver writes no segment before its frame is accepted, and installs the file only when the close's root and length match every segment. Fuzzed |
| A live segment is fetched before the frame that names it is readable | The provider adds a segment's root to its allowlist in the same step that pushes the signed frame, so a frame a receiver can see always names fetchable bytes, and nothing else is added. Segment bytes go over the static ALPN under the same policy |
| A write to standard output or a byte range leaves content in a shared temporary folder | Both stores live under the private `$SLUICE_HOME/tmp`, which is made 0700 on Unix and with the home's private access list on Windows, and does not take the home lock, so a receive still runs beside a send. The free space is checked there against the proved size before the body, a raw blob's size is proved by its first chunk group, and the folder is removed on every exit, not only on success |
| A resume reads or writes through a link at a target | A target is checked with `symlink_metadata` and a link is a clash, never read or followed. The size is compared before any content, the hash streams instead of reading a whole file into memory, and time and mode are set through a handle opened without following a link (`O_NOFOLLOW` on Unix, `FILE_FLAG_OPEN_REPARSE_POINT` on Windows) |
| `doctor --network` contacts more than the local machine | Only the relay named by `--relay`, or n0's public relays with `--n0-relays`; without one it sends nothing and says that two hard NATs cannot connect. The checks only read and change no setting, and a failed one is a warning line, not a failure of the command |
| A ticket holder fills the sender's disk | `iroh-blobs` push is disabled, and the `get`, `get_many` and `observe` modes are set together to the ticket's allowlist. Restricting `get` alone would leave the other two open. Tested |
| A leaked relay URL is used as free bandwidth | Relays run with authorization on. `open` mode needs a flag and has a per-client byte rate limit. ADR 0011, enforced by the guard below |
| An operator's relay config that opens more than it reads as: `access` left out or misspelled, any key iroh-relay would ignore, a missing config file, HTTPS turned off with `dangerous_http_only`, metrics left on by default with no login, a secret in the HTTP access URL, which iroh-relay logs, or no `[tls]` without `--behind-proxy` | Both images start through `sluice-relay-guard`, which refuses each of these with exit code 64 and one line. The line prints no value, and names a key only if it is 1 to 32 lower-case letters or `_`, and an argument only if it is `--` and 1 to 32 lower-case letters or `-`, so a token pasted in the wrong place stays out of the log. A `--config-path` value is printed only when it starts with `/` or `./`, is at most 200 characters and uses only `[A-Za-z0-9._/-]`; any other is "the path given to --config-path". An open relay starts only with `--open` and a per-client byte limit, and a relay with no `[tls]` only with `--behind-proxy`. ADR 0019, ADR 0020 and ADR 0021, [docker/guard](../docker/guard/README.md). Tested in each image on every build, fuzzed, and fuzzed against a mirror of iroh-relay's own reading |
| The relay config is replaced between the guard's check and the relay's start, as a Kubernetes ConfigMap update does, or is written in place so that it is empty for an instant | The guard copies the bytes it checked into a `memfd`, seals it against a write, a growth, a shrink and a further seal, puts that copy on descriptor 0 and hands iroh-relay `--config-path=/proc/self/fd/0`. A rename, an edit in place and a write that truncates the file first, as `cat >` does, all leave the copy alone; an empty config is iroh-relay's open default, which is why the window mattered. Before the exec, the guard checks that `/proc/self/fd/0` names the copy by device and inode and reads the checked bytes back through it, and exits 69 and starts nothing if it cannot. ADR 0021 |
| A relay config with no `[tls]`, and no `--behind-proxy` | Refused in closed and open mode: iroh-relay then serves plain HTTP, and a shared token or HTTP bearer token would cross the network in clear text. `--behind-proxy` with a `[tls]` table, or with `--rendezvous`, is refused too, so the flag and the file agree. Tokens then cross the hop from the proxy to the relay in clear text, so that hop must stay on one host or a private network. ADR 0021, relay README |
| Another local user reaches the identity key through the home folder, plants a key or a link, or turns the store's files onto another file | A new or empty home is made private through one handle opened without following a link: mode 0700 on Unix, read back because some mounts accept `chmod` and change nothing, and a private access list on Windows. On every open, not only a fresh one, the home and each of `blobs`, `db` and `logs` are checked for their owner through a handle that does not follow a link, and one another user owns is refused, because its owner can undo what sluice sets. Root on Unix, and `Administrators` and `SYSTEM` on Windows, count as this user: they can already do anything to the files, so accepting them adds no risk, and a home a backup tool restored still opens. `identity.key` is opened without following a link and without waiting for a writer, so a FIFO at the name is refused as not a regular file rather than waited on, and refused unless it is a regular file, has one name, and is owned by this user; a second hard link is a second way to change the key. `sluice id --rotate` replaces a key this user cannot open, as mode 0000 makes it: the new key goes to a temporary file and is renamed over the name, so nothing is written through the old key, and the entry is checked by name for the same four refusals. The second look after making an empty home private accepts an entry only when this user made it, read the same way, and never accepts `identity.key` or `identity.key.tmp`, so a planted key is a policy error. `lock` and `lock.pid` are opened without following a link and without waiting, and must be plain files: a FIFO, a device, a folder or a junction is refused and left where it is. Every file call uses the path the system resolved, in its verbatim form on Windows. When the file system cannot resolve a path, the longest ancestor it can resolve is used, every component from that ancestor down to the home is kept and checked for a link, so a home under a link the system made, such as macOS's `/var` or FreeBSD's `/home`, opens, while a link below that ancestor is refused by name. `sluice doctor` checks the same: the home's owner, each folder's owner, and the key's owner, kind and number of names, the mode or list, and that `blobs`, `db` and `logs` are folders and not links. Tested on both platforms, except a symbolic link on a Windows user without the right to make one, where a junction stands in |
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
- **A macOS extended ACL on the home.** `chmod 0700` does not remove an ACL entry that gives another
  user access, and neither `open` nor `doctor` reads the ACL. A home that came from somewhere else
  should be looked at with `ls -le` before it is used.

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
after the `release` environment's approver approves it. **The owner can release alone:** the tag
ruleset's bypass list names Repository admin, the owner can tag any commit whose `release.yml` names
no environment, and the owner can change the settings that make the review real: tick the bypass
back on, remove the second approver, or untick prevent self-review. The approval protects against a
release started by anyone else and against an owner's mistake, a tag off `main` or a `.github/`
changed on the tag, and nothing more. The approver has two commands for the second case, in
[the release runbook](runbooks/releasing.md), and they run only when a review is asked
for. Releases are immutable, so the assets and the tag cannot change after publication. The
workflow creates the
release as a draft, adds every file and publishes last; `gh release create` already does that itself
at 2.102.0, and the workflow writes the steps out so a failure leaves a draft it names, which `host`
refuses to start over until it is deleted. A failed build publishes nothing, but a failed image push
leaves the tags [the release runbook](runbooks/releasing.md) names for each step until the job is
re-run. Each image is checked by the ID its build job recorded in a job output before syft ran, so a
tarball
swapped in after the build is refused before a tag or a push, and the version index is built from
the two digests that were attested, never from a tag another push can move. **ghcr.io cannot limit
which workflow pushes to a package:** it grants Actions access per repository, not per workflow
file, so any workflow in a repository with access and `packages: write` can push. An image is
trusted only after its attestation passes `gh attestation verify` and it is pulled by that digest.
[the release runbook](runbooks/releasing.md) lists every file and says how to check one. What a
release has:

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
  the maintainer does not have yet: Azure Artifact Signing, and an Apple Developer ID. The release
  runbook says the same under **Not done yet**.
- **The installers verify no signature,** and the PowerShell one no checksum either. Until they do,
  the way to check a download is `gh attestation verify` on the archive, as
  [`docs/install.md`](install.md) shows, before running anything from it.
