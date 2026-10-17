# Security

## Reporting

Report privately through GitHub Security Advisories on this repository, or by email to the address in
the repository profile. Do not open a public issue for a vulnerability.

Expect an acknowledgement within 72 hours and an assessment within seven days. Coordinated disclosure,
with credit unless you prefer otherwise.

## Threat model, in short

**A ticket is a secret.** Anyone holding one can read the content it names. Tickets are capabilities,
not identifiers. Treat them like a password in every channel you send them through.

**What the design protects against**

- A malicious relay. Relays route ciphertext, cannot decrypt it, and cannot inject bytes, because every
  16 KiB chunk group is verified against the BLAKE3 root before it is exposed.
- A malicious provider in a multi-provider fetch. Same verification. Providers serving failing ranges
  are scored down and dropped.
- Redirection to the wrong peer. Endpoints are dialed by Ed25519 public key, not by address.
- Passive recording for later quantum decryption, **on native builds only**. Hybrid key exchange
  (`X25519MLKEM768`) requires the `aws-lc-rs` rustls provider with `prefer-post-quantum`, which the
  native binary pins. The browser build cannot use that provider and negotiates classical X25519. The
  session panel names which one is in use rather than leaving you to guess.
- Hostile archive paths. Absolute paths, `..`, symlinks escaping the output root, NTFS alternate data
  streams, and Windows reserved names are rejected by the receiver and covered by a fuzz target.

**What it does not protect against**

- **Identity forgery by a future quantum adversary.** Confidentiality is post-quantum where supported;
  identity is Ed25519 and is not. Post-quantum signatures are tracked, not shipped.
- **A compromised endpoint.** If the identity key is stolen, that device's identity is stolen.
- **Traffic analysis.** A relay sees that two endpoints exchanged a volume of bytes at a time. This
  matters more for browser sessions than native ones, because browser paths are always relayed.
- **An unauthenticated relay you expose.** A relay URL in a public repo, a client bundle, or a
  screenshot is a bandwidth faucet. Our container ships with authorization on; `open` mode exists, needs
  an explicit flag, and is rate limited even then.
- **Anyone you gave a ticket to.** There is no revocation beyond expiry and one-time use.

## Known caveats, stated up front

- **`spake2` has never had an independent audit**, by its own README. It is used only for optional short
  codes, which are off the default path and rate-limited at the rendezvous. If you need maximum
  assurance, share full tickets instead.
- **Identity rotation is destructive.** A new key means a new endpoint id, which invalidates every
  pairing and every unexpired ticket naming the old id.
- **Relay operators forward ciphertext they cannot inspect.** If you run an open relay you cannot
  moderate what passes through it. The default is an allowlist for exactly this reason.

## Supply chain

Releases are built in CI with GitHub Artifact Attestations, producing SLSA provenance and a signed SBOM
in a public transparency log. Verify with `gh attestation verify` or `cosign`. Binaries carry their own
dependency list via `cargo-auditable`. `cargo-deny` blocks known advisories and unclear licenses in CI.

Installer scripts verify a signature before executing anything. If you are asked to pipe an unsigned
binary into a shell from anywhere claiming to be this project, it is not this project.
