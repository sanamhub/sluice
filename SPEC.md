# Sluice Protocol Specification

**Version:** 0.1 draft. Nothing here is stable until 1.0.
**Status:** written before the implementation. Code follows this file, not the reverse.

This document is written so a third party can build a compatible implementation without reading our
source. If something here is ambiguous, that is a bug in this file.

---

## 1. Scope

Sluice moves content between two endpoints over QUIC. It defines four things:

1. **Static content transfer.** Delegated to the `iroh-blobs` protocol. Not redefined here.
2. **Live content transfer.** A signed segmented append-only log, for content still being written.
3. **Delta hints.** An optional negotiation that lets a receiver reconstruct ranges locally.
4. **Container formats.** Tickets, directory manifests, small-file packs, receipts.

The transport, NAT traversal, and relay behaviour come from `iroh` and are out of scope here.

---

## 2. Conventions

- All integers are little-endian unless stated.
- All hashes are BLAKE3-256, 32 bytes.
- All signatures are Ed25519, 64 bytes, over the byte sequence stated at each use.
- All variable-length fields are length-prefixed with a `varint` (LEB128, unsigned, max 10 bytes).
- Paths in manifests are UTF-8, NFC-normalized, forward-slash separated, relative, and never begin with
  `/` or contain a `..` component, and pass the receiver rules in section 8.
- Timestamps are UTC milliseconds since the Unix epoch, as `u64`.
- "MUST", "SHOULD", "MAY" carry their RFC 2119 meaning.

---

## 3. ALPN and negotiation

| ALPN | Purpose |
|---|---|
| `iroh-bytes/...` | Static content. Owned by `iroh-blobs`, version pinned by the release |
| `sluice/1` | Everything defined in this document |

On a `sluice/1` connection the dialer opens a control stream and sends `Hello`, and the listener replies
with `Hello`.

```
Hello = {
  proto_version: u16,          // 1
  capabilities: u64,           // bitset, see below
  max_frame_bytes: u32,        // receiver's limit, sender MUST respect
}
```

| Bit | Capability |
|---|---|
| 0 | `LIVE` |
| 1 | `DELTA` |
| 2 | `PACKS` |
| 3 | `MANIFEST_V1` |
| 4 | `RECEIPTS` |
| 5 | `ZSTD` |

Unknown bits MUST be ignored, never treated as an error. A capability is usable only when both sides set
it. A `proto_version` mismatch MUST close the connection with a clear reason string.

A dialer MUST offer exactly one ALPN from this table per connection, so the order in which a listener
registers or prefers them never decides which protocol runs. Static content and `sluice/1` use separate
connections. A listener MAY close a connection that offers more than one.

Wire-breaking changes bump the ALPN to `sluice/2`. Additive changes take a new capability bit.

---

## 4. Ticket

A ticket is the complete address of transferable content. **A ticket is secret material.** Anyone
holding one can read the content it names.

```
Ticket = {
  version: u8,                 // 1
  kind: u8,                    // 0 = Static, 1 = Live
  endpoint_id: [u8; 32],       // Ed25519 public key of the provider
  addrs: [SocketAddr],         // 0..n direct addresses, may be empty
  relay_url: Option<String>,   // relay to reach the provider, may be absent
  expiry: Option<u64>,         // UTC ms, absent means no expiry
  body: StaticBody | LiveBody,
}

StaticBody = { root: [u8; 32], size: u64, format: u8 }   // format: 0 = raw, 1 = hash sequence
LiveBody   = { stream_id: [u8; 16], sender_key: [u8; 32] }
```

**Bytes.** The ticket is written as bytes, then as text. The little-endian rule of section 2 does not
apply inside a ticket: its bytes follow the `postcard` 1.x encoding as stated here, so a third party
needs no copy of postcard. The fields come in this order, with no padding and no tag except where the
table says so:

| Field | Bytes |
|---|---|
| `version` | 1 byte, `0x01` |
| `kind` | 1 byte, `0x00` for Static or `0x01` for Live |
| `endpoint_id` | 32 bytes, no length prefix |
| `addrs` | the count as a varint, then that many addresses |
| one address | a tag byte, `0x00` for IPv4 or `0x01` for IPv6, then the 4 or 16 address bytes in network order, then the port as a varint. No flow label and no scope id |
| `relay_url` | `0x00` if absent. Otherwise `0x01`, the length in bytes as a varint, then that many bytes of UTF-8 |
| `expiry` | `0x00` if absent. Otherwise `0x01`, then the value as a varint |
| body, Static | `root` as 32 bytes, `size` as a varint, `format` as 1 byte |
| body, Live | `stream_id` as 16 bytes, then `sender_key` as 32 bytes |

The body has no tag of its own: `kind` selects it. A varint is LEB128: 7 bits per byte, the lowest
group first, the high bit set on every byte but the last, and at most 10 bytes for a `u64` and 3 for a
port. An encoder MUST write the shortest varint that holds the value.

**Text.** The bytes are written as Crockford base32 without padding, after the prefix `sluice1`. The
alphabet is `0123456789abcdefghjkmnpqrstvwxyz`, which leaves out `i`, `l`, `o` and `u`. Each character
carries 5 bits, taken from the bytes most significant first. If the last character is not full, its
unused low bits MUST be zero. The encoder writes lower case. The result is URL-safe and double-click
selectable.

A decoder reads the prefix and the body without regard to case. It MUST reject any character outside
the alphabet, including `-`, `=`, white space, and `I`, `L`, `O` and `U` in either case, so one ticket
has one spelling apart from case. Crockford's own rules would read `I` and `L` as `1`, `O` as `0`, and
skip `-`.

**Decoding.** A decoder MUST reject a ticket when:

1. the text is longer than 4096 bytes. This is checked before any decoding
2. the prefix is missing, a character is outside the alphabet, the body length is 1, 3 or 6 characters
   more than a multiple of 8 (whole bytes cannot give it), or the unused bits of the last character
   are not zero
3. `version` is not 1, `kind` is not 0 or 1, or `format` is not 0 or 1
4. bytes remain after the body, or the bytes end before it does
5. an `Option` or address tag is not one of the two values in the table, `relay_url` is not UTF-8, or
   a varint is longer than the shortest one for its value or does not fit its integer
6. `relay_url` is present and empty. Absence is `0x00`, and a second spelling of it is not allowed

**Size.** Two IPv4 addresses on ports from 16384 up, no relay, no expiry and a Static body of
100 MiB make 90 bytes, which is 151 characters with the prefix. With two IPv6 addresses it is 114
bytes and 190 characters. A Live ticket with the IPv4 pair is 101 bytes and 169 characters.
Implementations MUST accept tickets up to 4096 bytes of text and MUST NOT write longer ones.

**Short codes.** Where a rendezvous is configured, a ticket MAY be published under a short code. The
code is a SPAKE2 password; the rendezvous stores only the ciphertext and MUST rate-limit attempts per
code. Short codes are optional, and the SPAKE2 implementation in use has no independent audit, which
implementations MUST state in their documentation.

---

## 5. Static content

Delegated to `iroh-blobs`: BLAKE3 content addressing, 16 KiB chunk groups, verified range requests, and
hash sequences for collections. This specification pins the exact `iroh-blobs` protocol version per
release and does not restate it.

Two rules are ours:

1. A range response MUST be verifiable against the root before any byte is exposed to the caller, either
   through the application API or through a mounted filesystem.
2. Chunk group size MAY be negotiated to 64 KiB for blobs above 8 GiB, to reduce outboard overhead from
   about 0.4% to about 0.1% of content size. The choice is recorded in the request.
3. A provider MUST refuse every `iroh-blobs` push request. Content reaches a provider's store only
   because its own user added it.
4. A provider MUST apply one access policy to `get`, `get_many` and `observe`. A request that one of
   them would refuse, the other two refuse as well, so no request type is a way around the policy.

---

## 6. Live content

A growing file has no stable BLAKE3 root, because a bao tree's shape depends on total length. Live
content is therefore a chain of independently verifiable segments.

```
SEGMENT_BYTES = 8 MiB  (default; the provider announces the value in LogOpen)

LogOpen  = { stream_id: [u8;16], sender_key: [u8;32], segment_bytes: u32, started: u64 }
LogEntry = { seq: u64, segment_root: [u8;32], len: u32, prev_entry_hash: [u8;32] }
Frame    = { entry: LogEntry, sig: [u8;64] }
LogClose = { final_root: [u8;32], total_len: u64, closed: u64, sig: [u8;64] }
```

Rules:

- `seq` starts at 0 and increases by exactly 1. A gap MUST abort the stream.
- Two validly signed frames with the same `seq` and different `LogEntry` encodings mean the sender
  signed two histories, through a leaked key or a bug. A receiver MUST abort the stream as a
  verification failure that names the `seq`, MUST keep only the segments it verified before that `seq`,
  and MUST NOT accept an entry at or after it. A frame byte-identical to one already accepted is a
  repeat, not a fork, and MAY be dropped.
- `prev_entry_hash` is BLAKE3 of the previous `LogEntry` encoding. For `seq = 0` it is BLAKE3 of the
  `LogOpen` encoding, which binds the chain to the stream identity.
- `sig` covers the `LogEntry` encoding, under `sender_key`. A receiver MUST verify every signature.
- `len` equals `segment_bytes` for every entry except the last, which MAY be shorter.
- Segment content is fetched by `segment_root` using the static protocol, so range requests and resume
  work inside a segment.
- **The tail.** A provider MAY serve the incomplete final segment at its current length, with its own
  tree over that prefix. Its `segment_root` changes as it grows and is therefore not part of the signed
  chain until the segment is sealed. A receiver MUST treat tail bytes as provisional and MUST re-verify
  against the sealed `segment_root` when the entry arrives.
- `LogClose` publishes the whole-file BLAKE3 root, after which the content is ordinary static content
  and is addressable and dedupable as such.
- A receiver that joins late MUST be able to request the chain from `seq = 0`.

**Clock.** `started` and `closed` are informational. No verification depends on them, because peer
clocks disagree.

---

## 7. Delta hints

Optional, capability `DELTA`. Purpose: resending a large file that changed slightly should not move the
whole file.

```
SigList = { algo: u8, avg_bytes: u32, entries: [SigEntry] }   // algo 1 = FastCDC v2020, 64-bit
SigEntry = { len: u32, strong: [u8; 16] }                     // BLAKE3-128 of the chunk
NeedRanges = { root: [u8;32], ranges: RangeSet }
```

Flow:

1. The **provider** sends `SigList` for the content the requester is already authorized to read.
   Signatures flow provider to requester, never the reverse, so the requester discloses nothing about
   its local files.
2. The requester chunks its own candidate files with the same parameters, matches strong hashes, and
   computes which byte ranges of the target it can reconstruct locally.
3. The requester sends `NeedRanges` for the remainder and fetches those with the static protocol.
4. The requester assembles and MUST verify the whole result against `root`. On failure it MUST discard
   the assembly and fall back to a plain fetch.

Delta is a hint layer. No verification outcome may ever depend on it.

---

## 8. Directory manifest

Capability `MANIFEST_V1`. A directory is a manifest blob plus the blobs it references.

```
Manifest = { version: u16, entries: [Entry], packs: [PackRef] }

Entry = {
  path: String,                // relative, NFC, forward slashes
  kind: u8,                    // 0 = File, 1 = Dir, 2 = Symlink
  size: u64,
  mtime_ms: u64,
  mode: u32,                   // Unix mode bits; 0 when unknown
  loc: Loc,
  link_target: Option<String>, // present when kind is Symlink, absent otherwise
}

Loc = Blob { root: [u8;32] }
    | Packed { pack_index: u32, offset: u64, len: u64 }
    | None                     // Dir and Symlink
```

Rules:

- Entries MUST be sorted by `path`, byte-wise, so a manifest is deterministic and dedupable.
- Empty directories MUST be preserved as `Dir` entries.
- `mtime_ms` is restored by default. Implementations MUST offer a way to skip it.
- `mode` is applied on Unix. On Windows only the read-only bit is honored.
- **Symlinks are recorded and never followed during send.** A receiver MUST NOT create a symlink unless
  explicitly enabled.
- `link_target` is present exactly when `kind` is Symlink. A receiver MUST reject an entry where it is
  missing on a Symlink or present on anything else.
- A receiver MUST reject a `link_target` when:

  1. it is empty, or longer than 4096 bytes in UTF-8
  2. it begins with `/`, or ends with `/`, so an absolute target is never accepted
  3. a component is empty or `.`. A `..` component is allowed, within rule 5
  4. it breaks path rule 2, 5, 6, 7 or 8 below, applied the same way. Rule 6 rejects `\` and `:`, so
     `C:\x` and `\\server\share` are rejected too
  5. resolved from the folder that holds the link, one component at a time, a `..` would step above
     the output root. So `a/link -> ../b` is accepted and `link -> ../x` is rejected

  These rules look at the text only. A target that passes them can still reach another symlink on
  disk, which is the writer's concern, as for paths.
- Hard links are not preserved. Implementations MUST document this.
- A receiver MUST reject a manifest path, on every platform, when:

  1. it is empty, or longer than 4096 bytes in UTF-8
  2. it is not in Unicode NFC. A receiver does not normalize it, because then two spellings would name
     one file
  3. it begins with `/`, or ends with `/`
  4. a component is empty (`a//b`), `.` or `..`
  5. a component is longer than 255 bytes in UTF-8
  6. it contains `\`, `:`, a control character (U+0000 to U+001F, or U+007F to U+009F), a
     bidirectional control (U+061C, U+200E, U+200F, U+202A to U+202E, or U+2066 to U+2069), or any of
     `< > " | ? *`
  7. a component ends with `.` or a space
  8. a component is a Windows reserved device name, compared case-insensitively, with or without an
     extension: `CON`, `PRN`, `AUX`, `NUL`, `CONIN$`, `CONOUT$`, and `COM` or `LPT` followed by one of
     `0` to `9`, `¹`, `²` or `³`. The name is the text before the first `.`, spaces before that `.`
     are ignored, and U+0131 is compared as `I`. So `con`, `Nul.txt` and `LPT¹.log` are rejected, and
     `CONSOLE` is not

  With these rules no accepted path can leave the output root by its text alone, and a symlink already
  on disk is the writer's concern, not these rules'.

---

## 9. Small-file packs

Capability `PACKS`. Per-file overhead dominates on trees of many small files, which is the workload that
makes transfer tools look slow.

- Files smaller than `PACK_THRESHOLD` (default 1 MiB) are concatenated into pack blobs of at most
  `PACK_TARGET` (default 64 MiB).
- Packing order is manifest order, which is sorted by path, so packing is deterministic and identical
  inputs dedup across transfers and across senders.
- A file is never split across packs.
- Each pack is an ordinary blob with its own BLAKE3 root, so ranges, resume, and verification work.
- `PackRef = { root: [u8;32], size: u64 }`, indexed by `pack_index` in manifest entries.
- A receiver MUST stream out of a pack while fetching it, never staging a whole pack in memory.

---

## 10. Receipt

Capability `RECEIPTS`. A sender-signed statement that specific content was offered.

```
Receipt = { root: [u8;32], total_len: u64, sender_key: [u8;32], utc_ms: u64, sig: [u8;64] }
```

`sig` covers the first four fields. A receipt proves the sender offered that content at that time. It
does not prove delivery, and implementations MUST NOT describe it as proof of delivery.

---

## 11. Control API

The local control interface is defined by `proto/sluice/v1`, governed by `buf`, and served over a Unix
socket or Windows named pipe. An implementation MAY also serve it over TCP, and then MUST require TLS and
token authentication.

It is a control plane. **No file bytes ever traverse this API.** A client asks the engine to move bytes
and observes progress; it never carries them.

Store access is single-writer. A client MUST take an exclusive lock on the store directory, and if the
lock is held by a live process it MUST connect to that process instead of opening the store.

---

## 12. Test vectors

`tests/vectors/` contains, for each release:

| Vector | Contents |
|---|---|
| `tickets/` | Encoded tickets with expected decoded fields, including malformed ones that MUST be rejected |
| `manifests/` | Directory trees with expected manifest bytes and root hash |
| `packs/` | Small-file trees with expected pack boundaries and offsets |
| `live/` | Segment chains with expected entry hashes and signatures, plus chains that MUST fail |
| `delta/` | File pairs with expected chunk boundaries and resulting need-ranges |
| `paths/` | Hostile paths that MUST be rejected by a receiver |

`cargo xtask conformance` runs every vector against the local implementation. A third-party
implementation that passes these vectors is compatible with ours.

---

## 13. Versioning policy

- This document is versioned independently of the product.
- Wire-breaking changes bump the ALPN. Additive changes take a capability bit.
- CI runs interop of the current build against the previous release and the oldest supported release.
- The on-disk store layout is versioned separately in `$SLUICE_HOME/VERSION`, with forward-only
  migrations. An older binary MUST refuse a newer layout rather than write to it.

---

## 14. Open items before 0.2

1. Confirm the exact `iroh-blobs` protocol version to pin, and whether its pre-1.0 ALPN change has landed.
2. Measure the segment size curve. 8 MiB is a guess trading latency against signature overhead.
3. Measure the pack size curve. 64 MiB and 1 MiB are guesses.
4. Decide whether `zstd` applies per chunk group or per pack. Per pack is simpler and probably enough.
5. Decide whether the tail-serving rule needs an explicit provisional-length field on the wire.
6. Settle section 5 rule 2. `iroh-blobs` fixes its block size at 16 KiB, so a 64 KiB chunk group cannot
   be negotiated while static content goes through it. Either the rule goes, or the block size becomes
   configurable upstream.
7. Add vectors to `live/` for the repeated-`seq` rule in section 6. The rule is set; no implementation
   reads live frames yet.
