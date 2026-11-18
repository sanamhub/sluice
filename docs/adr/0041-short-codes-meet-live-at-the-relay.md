# 0041. A short code is a live meeting at the relay, and a wrong guess burns it

**Status:** Accepted **Date:** 2026-11-18 **Approved by:** @sanamhub **Relates to:** ADR 0001, ADR 0011, ADR 0019, ADR 0028, ADR 0032, ADR 0039, ADR 0040

## Context

`SPEC.md` section 4 says a ticket may be published under a short code, that the code is a SPAKE2
password, and that "the relay stores only the ciphertext and MUST rate-limit attempts per code".
No component can do that today:

- `iroh-relay` 1.4 forwards packets between endpoint ids and stores nothing. It has no mailbox, no
  code, and no per-code counter.
- The `rendezvous` image is gone (ADR 0032). There is no second server to put a mailbox in.
- ADR 0040 says the project's own relay "stores nothing and logs no transfer". A stored ciphertext
  mailbox would make that sentence false, and would make the relay a place where a ticket outlives
  the connection that published it.

A code also cannot be a ticket with a shorter spelling. A ticket is 128 bits or more of key
material; a code a person can read over a phone is a few words. If the relay held the ticket under
a key derived from those words, anyone who saw the ciphertext could test guesses offline at the
speed of the hash: 16 bits is a fraction of a second, and even 22 bits is not a secret at rest.
The only way a short code can be safe is the way magic-wormhole does it: the two sides meet while
both are online, run a password-authenticated key exchange, and the relay never holds a testable
form of the password. The sender is online anyway, because it is the peer that serves the content.

The browser tab of ADR 0040 must be able to use a code, and a tab can open no socket but a
WebSocket, over HTTPS, to the relay the website names. So the meeting place has to be reachable
from a browser, at the relay's own origin.

A spike answered the three questions the design depended on (branch `spike/short-codes`, never to
be merged):

| Question | Answer, with the evidence |
|---|---|
| Can the meeting place share the relay's host and port? | Yes. `iroh-relay` 1.4.1's library exports `RelayService`, `Handlers` and `TlsConfig`, and `RelayService::handle_connection` accepts a TLS stream. A binary of ours registers `GET /meet` in `Handlers`, builds the relay service, and serves both paths on one listener. The spike's test connects two `iroh-relay` clients through `/relay` (a Ping/Pong and a Datagram from one to the other) and claims a nameplate over `wss://…/meet` on the same port and certificate, in one run. The upstream `Server` itself cannot add a route: its `ServerBuilder` is `pub(super)`. |
| Does `spake2` build for the browser? | Yes, on the newest stable release. `spake2` 0.4.0 with `curve25519-dalek` 4.1.3, `sha2` 0.10.9 and `getrandom` 0.2.17 builds for `wasm32-unknown-unknown` when `getrandom` 0.2 has its `js` feature, which the probe enables and the product build does not. The `.wasm` module grows by 56 650 bytes raw and 9 432 bytes with brotli, 0.8% and 0.7% of the module. The price of the stable release is a second `curve25519-dalek` (4.1.3 beside 5.0.0) and a second SHA-2 (0.10.9 beside 0.11.1); the pre-release variant measured 36 320 and 8 440 bytes, so stability costs 20 330 bytes raw and 992 with brotli. |
| Does the exchange work, including the burn? | Yes. Four in-process tests: right words deliver a ticket; wrong words burn the nameplate and the sender sees `Burned`; a third connection to a held nameplate is refused with `Busy`; and the relay keeps relaying on the same port. |

The spike also fixed the wire shape and the failure rules below; the numbers in it are the ones this
ADR decides.

## Decision

### 1. The meeting is live, and the relay stores nothing

Two connections meet at a nameplate. The meeting place forwards a few messages between them and
keeps no record once either closes. There is no mailbox, no queue for an offline peer, and no
retry after the sender goes away. A nameplate is released when the sender's connection closes, and
in every case after 10 minutes. A receiver that disconnects before it sends `Done` burns the
nameplate, because it may have used the one guess.

The meeting place runs in the relay image (ADR 0028) on the relay's host, so a user who configured
a relay has a meeting place with no new setting. It is served from the same process, the same TLS
listener and the same certificate as `/relay`, at `GET /meet` upgraded to a WebSocket. The image's
binary becomes ours: it reads the same `relay.toml` the guard checked, builds `RelayService` with
the `/meet` handler, runs `handle_connection` on the HTTPS address, and keeps QUIC address
discovery on its own port through the library's QUIC server. The guard's check and the binary's
reader stay one schema, as ADR 0019 requires; the guard execs `sluice-relay` instead of
`iroh-relay`.

The meeting place adds 171 520 bytes to a relay server binary of the same shape, about 1.8% of the
upstream `iroh-relay` 1.4.1 binary (9 650 688 bytes). The spike binary is smaller overall because it
does not yet carry the config parser, metrics, `clap` or the QUIC server; the image keeps those, so
the image grows by about the meeting place's 172 KB. The spike also starts the library's QUIC-only
server (`Server::spawn` with its relay half `None`) on a loopback port with the same TLS config and
shuts it down cleanly, so one process can serve 443 and 7842.

T89's binary reads the config the guard checked; the guard itself keeps its schema and its rules,
and its exec target becomes `sluice-relay`. The guard refuses a `[meet]` that enables the meeting
place without the three limits, and the binary refuses to serve `/meet` at all when `[meet]` is
absent. The binary also keeps the relay's small endpoints, which upstream registers before
`/relay`: `/`, `/index.html`, `/ping`, `/robots.txt`, `/healthz`, and the plain-HTTP
`/generate_204`. The relay config surface is the other half of the work. The two shipped recipes
use `cert_mode = "Manual"`, and T89 refuses `cert_mode = "LetsEncrypt"` when `[meet]` is enabled,
with a line that names the limit. That refusal is the decision; carrying ACME into `sluice-relay`
is later work, not an open choice. The config surface, and not the meeting place, is the largest
implementation risk, and T89 carries it in the same binary.

### 2. The code is `<nameplate>-<word>-<word>`

For example `7-orbit-velvet`.

- The meeting place hands out the nameplate: a decimal number from 1 to 9999, unique while it is
  held. It is written without leading zeros.
- The two words come from the English word list of BIP-0039, 2048 words in their published order,
  MIT licensed, pinned by SHA-256
  `2f5eed53a4727b4bf8880d8f3f199efc90e58503646d9ff8eff3a2ed3b24dbda`. Each word carries 11 bits,
  so two carry 22.
- The words never reach the meeting place. It sees the nameplate and the SPAKE2 messages, not the
  password.

**Why not 256 words and 16 bits.** The proposal named the PGP word list as a candidate. It is not
public domain: the list is copyrighted by PGP Corporation and was licensed under the GNU Free
Documentation License in 2007. A GFDL table in an implementation under MIT or Apache-2.0 puts a
share-alike document license on that table, and the alternative of copying the selection without
its license is worse. BIP-0039 is MIT, is already the most widely implemented word list of its
size, and its words are chosen to be unambiguous at four letters. The cost of the larger list is a
longer code; the benefit is that one guess is 1 in 4,194,304 instead of 1 in 65,536. The rest of
the design, including the burn, is unchanged.

A code is case-insensitive on input and written in lower case. It has one spelling apart from case:
exactly two `-` separators, the words as the list spells them, and no other character. A parser
rejects a leading zero, a nameplate outside 1..=9999, a word not on the list, and any extra or
missing part. **A code is secret material**, like a ticket: it is not written to a log, an error
message, or a crash report, and an error names only the part that failed.

### 3. SPAKE2 on the words, and a wrong guess burns the code

- The sender is SPAKE2 side A and the receiver side B. The password is the two words joined by one
  `-`, exactly as the code spells them.
- Both identities bind the protocol label and the nameplate:
  `sluice short code v1/nameplate/<n>/sender` and `.../receiver`. The label is fixed; a future
  version uses a new label and a new WebSocket path.
- After `finish`, both sides hold the SPAKE2 key. The transcript is
  `"sluice short code v1" || nameplate:u32be || len(A):u32be || A || len(B):u32be || B`, where A is
  the sender's 33-byte message and B the receiver's.
- HKDF-SHA256 with the transcript as salt and the SPAKE2 key as input keying material derives three
  32-byte values, with info `confirm sender`, `confirm receiver` and `ticket`. Each side sends its
  own confirmation tag and checks the other's in constant time. A mismatch is a failed guess.
- The ticket travels sealed with ChaCha20-Poly1305 under the `ticket` key: a random 12-byte nonce,
  then the ciphertext and tag, with the transcript as associated data. The ticket text is at most
  4096 bytes, so the sealed value is at most 4124 bytes.
- One failed confirmation closes the nameplate for good. The meeting place cannot verify the tags
  itself, so the side that sees the mismatch sends `Fail`, and the meeting place burns the
  nameplate and tells both sides `Burned`. A receiver that disconnects before sending `Done` burns
  it too. The sender must make a new code; a code is never retried.
- An attacker therefore has one guess in 4,194,304 per code, and the guess is online: SPAKE2 gives
  an eavesdropper no way to test a guess against the transcript. The relay holds no offline-testable
  value: a guess costs a live connection, and that connection burns the code.

The AEAD and the KDF come from `ring`, which the browser build already uses for TLS, so the browser
adds no new cipher implementation. The new code is `spake2` 0.4.0, the newest stable release. It
brings `curve25519-dalek` 4.1.3 and `sha2` 0.10.9 beside the tree's 5.0.0 and 0.11.1, and it reads
entropy through `getrandom` 0.2.17, whose `js` backend the browser build enables only for the probe.
The 0.4.0 `start_a`, `start_b` and `finish` calls, the 33-byte messages and the identities are the
same as the pre-release's, so the transcript, the HKDF schedule and the AEAD above do not change.
`spake2` has no independent audit, by its own README; the docs and the threat model say so. The
choice of the stable release over the pre-release is deliberate: a security path that protects a
ticket should not ride on an unaudited pre-release, even though that would save 20 330 bytes raw
and 992 with brotli. The stable release costs 56 650 bytes raw and 9 432 with brotli in the module.

### 4. The meeting place lives with the relay and speaks WebSocket over HTTPS

- `wss://<relay-origin>/meet`, derived from the relay the user configured: the same host and port,
  `https` becomes `wss`, and the path is `/meet`. A tab reaches the same URL.
- One WebSocket binary message is one frame. Client frames are `0x01 Claim`, `0x02 Join` with a
  `u32` nameplate, `0x03 Spake`, `0x04 Confirm` (32 bytes), `0x05 Sealed`, `0x06 Fail` with a
  reason, `0x07 Done`; server frames are `0x81 Claimed`, `0x82 Joined` with the role, `0x83 Spake`,
  `0x84 Confirm`, `0x85 Sealed`, `0x86 Burned`, `0x87 PeerClosed`, `0x88 Done`, `0x8f Error` with a
  code. Text frames are refused. A frame is at most 4608 bytes; a larger one closes the connection
  with `TooLarge`.
- `Claim` allocates; `Join` attaches the one receiver; both are the only ways to name a plate.
  A third connection to a held plate gets `Busy`. A claim when the table is full gets `Full`; a
  claim or join over the per-IP rate gets `RateLimited`; a plate older than 10 minutes gets
  `Expired`.
- The server never parses the ticket, the words, or the SPAKE2 messages; it copies bytes between the
  two peers, with the role check that only the sender may send `Sealed`.

### 5. Limits at the meeting place, through the guard

- `[meet]` in `relay.toml`: `enable = true`, `rate_per_ip` (claims and joins per minute),
  `max_plates`, and `ttl_seconds`. The guard refuses `enable = true` unless the three limits are
  present and inside the bounds this ADR sets: `rate_per_ip` 1..=600, `max_plates` 1..=9999,
  `ttl_seconds` 60..=600. A config without `[meet]` serves `/relay` only and answers `/meet` with
  `404`.
- Per-IP: a token bucket for claims and joins, and a cap on concurrent `/meet` connections per IP.
- Global: at most `max_plates` plates, and at most one receiver per plate.
- A nameplate is released when the sender's connection closes, or after `ttl_seconds`, whichever is
  first. The TTL is at most 10 minutes.
- Messages have the 4608-byte cap; a ticket is at most 4096 bytes of text.
- No logs of codes, nameplates, tickets, SPAKE2 messages or sealed values, at any level. The
  meeting place logs a refusal by error code and nothing else; the nameplate is not in the line.
- The existing relay limits still apply to `/relay`; the meeting place is not a relay and carries
  only these few messages.

### 6. Off unless asked, on both sides

- `sluice send --code` claims a nameplate, prints the code next to the ticket, and serves the
  meeting while it serves the transfer. The flag is off by default; without it, no connection to
  `/meet` is made.
- `sluice recv <code>` recognizes the code form and joins that nameplate; otherwise its argument is
  a ticket as before. The receiver must name the same relay the sender used: the code carries no
  host. A join at another relay's meeting place finds no such nameplate, and the error says the
  code was not found at the relay `recv` is configured for, and that the sender may have used a
  different relay.
- `sluice doctor` prints the configured relay and the `/meet` URL it would use, and says that a
  code only works when both sides name that relay. If the relay has no meeting place, it says so.
- If `--code` cannot claim a nameplate because no relay is configured or the relay serves no
  `/meet`, the command stops before serving and exits **6** (policy). The message is one of these,
  and it names no URL, code or ticket:
  `--code needs a relay with a meeting place, and no relay is configured; set --relay or send without --code`
  or
  `--code needs a relay with a meeting place, and the configured relay does not serve /meet; add [meet] to it or send without --code`.
  If the claim itself is refused (`Busy`, `Full`, `RateLimited`, `Expired`), the command reports the
  relay's answer and exits non-zero before serving. A user who asked for a code gets a code or an
  error, not a transfer with the flag silently ignored.
- The project's own relay (ADR 0040) is the default once it exists. Until then a code needs a
  relay the user configured; there is no built-in meeting place.

### 7. Pairing tickets may travel the same way later

The ticket-shaped secret of ADR 0039 can be sealed with the same exchange, so
`sluice pair --short-code` is allowed as a later change. It is not part of T89, and no pairing
message is defined here.

### 8. The `spake2` warning stays

The crate has no independent audit. Implementations MUST state that in their documentation, as
`SPEC.md` already requires. The threat model's short-code row and its `spake2` weakness are
rewritten for this design: one online guess, a burn, no stored ciphertext, and no audit.

## Alternatives

- **A stored ciphertext mailbox at the relay.** The sender deposits the ticket sealed under a key
  derived from the code; the receiver fetches it later. Refused: the relay would store a testable
  form of a 22-bit secret, so an offline guess becomes trivial; it would also outlive the sender
  and contradict ADR 0040's "stores nothing". A high-entropy code would fix the guessing but is no
  longer short.
- **A key derived from the code, with no PAKE.** Encrypt the ticket under a KDF of the words and
  let the relay hold it. Refused for the same reason: anyone who sees the ciphertext can test all
  4,194,304 codes offline. SPAKE2 exists to make that test impossible without a live guess.
- **A separate meeting server.** A second image, host, certificate and setting, with the browser
  facing another origin and CORS. Refused: the relay is already the one host both sides name, the
  meeting place shares its TLS listener and certificate, and a second deployment is one more thing
  an operator must configure for a feature that is off by default.
- **Doing nothing.** Leave the SPEC sentence as a promise no component keeps, and ship 1.1 without
  codes. Refused: the promise is already in the specification and the roadmap, and the design above
  is buildable from public library APIs, as the spike shows.
- **Use the PGP word list under GFDL.** The proposal named it, and it is 256 words and 16 bits.
  Refused: the table is copyrighted by PGP Corporation and GFDL, so it would put a share-alike
  document license on a table in an MIT/Apache implementation, and every copy would carry its
  notice. The 22-bit BIP-0039 list is permissive and shorter to license, though its codes are
  longer.
- **Use `spake2` 0.5.0-pre.0.** It shares the tree's `curve25519-dalek` 5 and `sha2` 0.11, so its
  module is 20 330 bytes raw and 992 bytes brotli smaller. Refused: it is a pre-release of a
  security crate with no audit, and the path that protects a ticket should use a stable release.
  Revisit when a stable 0.5 ships; then the second curve library and its SHA-2 leave the graph.

## Consequences

- The relay image gains a binary of ours, and the guard's exec target becomes `sluice-relay`. The
  guard's schema and the binary's reader must move together; a test compares the keys they accept.
  The image grows by about 172 KB, the meeting place's own code plus its handler and state machine.
- An operator who uses `cert_mode = "LetsEncrypt"` cannot enable the meeting place yet: T89 refuses
  that combination with a line that names the limit, and ACME parity is later work. The two shipped
  recipes use `cert_mode = "Manual"`.
- The browser module grows by 56 650 bytes raw and 9 432 bytes with brotli, 0.8% and 0.7%. The AEAD
  and KDF are `ring`, already present; the new code is `spake2` 0.4.0 and the second
  `curve25519-dalek` 4.1.3 and `sha2` 0.10.9 it brings. The pre-release variant measured 36 320 and
  8 440; the difference is what a stable release costs.
- T89 takes `spake2 = "=0.4.0"`, an exact pin on the newest stable. The tree then carries
  `curve25519-dalek` 4.1.3 beside 5.0.0, `sha2` 0.10.9 beside 0.11.1 and `rand_core` 0.6.4.
  `cargo deny check` reports `bans ok`; this repository's `deny.toml` sets no multiple-version
  denial, so no skip entry is needed. If that policy ever changes, the skip names exactly
  `curve25519-dalek` 4.1.3 and `sha2` 0.10.9, with a comment naming this ADR and saying that it goes
  away when a stable `spake2` shares the tree's `curve25519-dalek` 5.
- A code is one guess, but a nameplate is public: anyone who learns or scans a live nameplate can
  join it first, fail the confirmation, and burn the code. That is denial of service, not
  disclosure; it is rate-limited per IP, the sender is told, and a new code is cheap. The threat
  model says so.
- The relay sees a nameplate, two connections, their timing and the sizes of the sealed frames. It
  does not see the words or the ticket. Traffic analysis remains out of scope, as before.
- A sender that closes before the receiver joins releases the nameplate and the code is dead; the
  receiver gets `PeerClosed`. A receiver that closes early burns the code; the sender gets `Burned`.
- Revisit if `iroh-relay` exposes a public route hook on its own `Server` (the `Handlers` route is
  enough today), if BIP-0039's license changes, or when a stable `spake2` 0.5 shares the tree's
  curve arithmetic, which lets the second `curve25519-dalek` and `sha2` go.
