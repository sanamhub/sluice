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
- Paths in manifests are UTF-8, NFC-normalized, forward-slash separated, relative, and never begin
  with `/` or contain a `..` component, and pass the receiver rules in section 8.
- Timestamps are UTC milliseconds since the Unix epoch, as `u64`.
- Sizes and rates written on a command line are bytes unless a unit follows: `K`, `M`, `G` and `T`
  alone or with `iB` are powers of 1024, the same letters with `B` are powers of 1000, and a bare
  number is bytes. Surrounding whitespace is not accepted: a size is read exactly as written, so a
  count cannot quietly mean something other than what it looks like. `--limit-rate`, `--max-size`
  and `--range` read sizes with the same parser.
- "MUST", "SHOULD", "MAY" carry their RFC 2119 meaning.

---

## 3. ALPN and negotiation

| ALPN | Purpose |
|---|---|
| `iroh-bytes/...` | Static content. Owned by `iroh-blobs`, version pinned by the release |
| `sluice/1` | Everything defined in this document |

On a `sluice/1` connection the dialer opens a control stream and sends `Hello`, and the listener
replies with `Hello`.

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

Unknown bits MUST be ignored, never treated as an error. A capability is usable only when both sides
set it. A `proto_version` mismatch MUST close the connection with a clear reason string.

A dialer MUST offer exactly one ALPN from this table per connection, so the order in which a
listener registers or prefers them never decides which protocol runs. Static content and `sluice/1`
use separate connections. A listener MAY close a connection that offers more than one.

Wire-breaking changes bump the ALPN to `sluice/2`. Additive changes take a new capability bit.

**Completion close.** The application close code `0x534C0001` on a static content connection means
"the whole transfer is here". A receiver MUST close its content connection with that code, and a
reason it may choose, exactly when it has verified and installed every file of the transfer, and it
MUST NOT send that code on a receive that failed, on a receive of a byte range, or on a receive that
writes only part of a transfer to standard output. A sender MAY stop serving a one-download ticket
when it sees the code, and it MUST NOT treat it as anything other than a finished download. It MUST
accept the code only from a connection that this process allowed a request from and that asked for
every content root the ticket names; a code from any other connection leaves the sender serving. The
code is not authenticated by the transfer, and the identity a sender's ticket names is persistent, so
without that rule a past holder of any of its tickets, or a host that learned its endpoint id from
mDNS, could end a send and have it report success. For a connection that asked for every root, the
code grants only what taking every byte already granted, because delivery is counted by the bytes
served and taking them ends the same sender. A sender that stops on the code has the receiver's word
for the receiver's disk and nothing else, so it stops without changing what it promised; a transfer
that ended early is finished by asking the sender for a new ticket. A sender that never sees the code
stops by counting bytes, as **Static content** and the provider implementation describe.

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

**Bytes.** The ticket is written as bytes, then as text. The little-endian rule of section 2 does
not apply inside a ticket: its bytes follow the `postcard` 1.x encoding as stated here, so a third
party needs no copy of postcard. The fields come in this order, with no padding and no tag except
where the table says so:

| Field | Bytes |
|---|---|
| `version` | 1 byte, `0x01` |
| `kind` | 1 byte, `0x00` for Static or `0x01` for Live |
| `endpoint_id` | 32 bytes, no length prefix |
| `addrs` | the count as a varint, then that many addresses |
| one address | a tag byte, `0x00` for IPv4 or `0x01` for IPv6, then the 4 or 16 address bytes in network order, then the port as a varint. No flow label and no scope id |
| `relay_url` | `0x00` if absent. Otherwise `0x01`, the length in bytes as a varint, then that many bytes of UTF-8, which rule 7 below limits to one spelling of an `https` or `http` URL |
| `expiry` | `0x00` if absent. Otherwise `0x01`, then the value as a varint |
| body, Static | `root` as 32 bytes, `size` as a varint, `format` as 1 byte |
| body, Live | `stream_id` as 16 bytes, then `sender_key` as 32 bytes |

The body has no tag of its own: `kind` selects it. A varint is LEB128: 7 bits per byte, the lowest
group first, the high bit set on every byte but the last, and at most 10 bytes for a `u64` and 3 for
a port. An encoder MUST write the shortest varint that holds the value.

**Text.** The bytes are written as Crockford base32 without padding, after the prefix `sluice1`. The
alphabet is `0123456789abcdefghjkmnpqrstvwxyz`, which leaves out `i`, `l`, `o` and `u`. Each
character carries 5 bits, taken from the bytes most significant first. If the last character is not
full, its unused low bits MUST be zero. The encoder writes lower case. The result is URL-safe and
double-click selectable.

A decoder reads the prefix and the body without regard to case. It MUST reject any character outside
the alphabet, including `-`, `=`, white space, and `I`, `L`, `O` and `U` in either case, so one
ticket has one spelling apart from case. Crockford's own rules would read `I` and `L` as `1`, `O` as
`0`, and skip `-`.

**Decoding.** A decoder MUST reject a ticket when:

1. the text is longer than 4096 bytes. This is checked before any decoding
2. the prefix is missing, a character is outside the alphabet, the body length is 1, 3 or 6
   characters more than a multiple of 8 (whole bytes cannot give it), or the unused bits of the last
   character are not zero
3. `version` is not 1, `kind` is not 0 or 1, or `format` is not 0 or 1
4. bytes remain after the body, or the bytes end before it does
5. an `Option` or address tag is not one of the two values in the table, `relay_url` is not UTF-8,
   or a varint is longer than the shortest one for its value or does not fit its integer
6. `relay_url` is present and empty. Absence is `0x00`, and a second spelling of it is not allowed
7. `relay_url` is present and is not an absolute URL with the scheme `https` or `http`, at most 2048
   bytes long, with a host, and with no user name, password, query or fragment. It MUST also equal
   its own serialization by the WHATWG URL standard, so it has one spelling: a lower-case scheme and
   host, `/` as the path when there is none (`https://relay.example/`, not `https://relay.example`),
   no default port, and a host in punycode. A decoder parses it with an implementation of that
   standard and compares the result with the text. The `url` crate is one, and the one iroh's
   `RelayUrl` parses with, so a ticket that decodes is one iroh accepts later
8. `endpoint_id`, or the `sender_key` of a Live body, is not an Ed25519 public key in its one
   encoding, or is a point of small order. It is checked last. The one encoding is the one that
   RFC 8032 section 5.1.3 decodes: `y` below `p` (2^255 - 19), and the sign bit not set when `x`
   is 0. A decoder that follows ZIP-215, as `ed25519-dalek` does, also takes the other spellings,
   so it decodes the point, encodes it again and compares the result with the 32 bytes. Without
   this, two decoders could disagree on whether one ticket is valid. No signature from a key of
   small order verifies strictly (section 6), so a ticket that names one could only lead to a
   provider or a stream that nothing can trust

**Rejected tickets.** A decoder MUST NOT write a rejected ticket, or any part of it, to a log, an
error message or a crash report. The error says which rule failed and nothing of the text. A
rejected ticket can be a real one, mistyped or cut short, and it is still a secret.

**Expiry.** A ticket is expired when the current UTC time in milliseconds is equal to or greater
than `expiry`, the same boundary as the `exp` claim of a JWT: at `expiry - 1` it is still good, and
at `expiry` it is not. A ticket with no `expiry` never expires. Expiry is not a decoding rule,
because decoding reads no clock.

**Size.** Two IPv4 addresses on ports from 16384 up, no relay, no expiry and a Static body of
100 MiB make 90 bytes, which is 151 characters with the prefix. With two IPv6 addresses it is 114
bytes and 190 characters. A Live ticket with the IPv4 pair is 101 bytes and 169 characters.
Implementations MUST accept tickets up to 4096 bytes of text and MUST NOT write longer ones.

**Short codes.** Where a rendezvous is configured, a ticket MAY be published under a short code. The
code is a SPAKE2 password; the rendezvous stores only the ciphertext and MUST rate-limit attempts
per code. Short codes are optional, and the SPAKE2 implementation in use has no independent audit,
which implementations MUST state in their documentation.

---

## 5. Static content

Delegated to `iroh-blobs`: BLAKE3 content addressing, 16 KiB chunk groups, verified range requests,
and hash sequences for collections. This specification pins the exact `iroh-blobs` protocol version
per release and does not restate it.

Two rules are ours:

1. A range response MUST be verifiable against the root before any byte is exposed to the caller,
   either through the application API or through a mounted filesystem.
2. Chunk group size MAY be negotiated to 64 KiB for blobs above 8 GiB, to reduce outboard overhead
   from about 0.4% to about 0.1% of content size. The choice is recorded in the request.
3. A provider MUST refuse every `iroh-blobs` push request. Content reaches a provider's store only
   because its own user added it.
4. A provider MUST apply one access policy to `get`, `get_many` and `observe`. A request that one of
   them would refuse, the other two refuse as well, so no request type is a way around the policy.

---

## 6. Live content

A growing file has no stable BLAKE3 root, because a bao tree's shape depends on total length. Live
content is therefore a chain of segments, each sealed with its own root when it is full, and each
entry of the chain signed by the sender. A receiver gets these records from providers it does not
trust, so every rule below is checked before a segment is fetched or written.

```
LogOpen  = { stream_id: [u8;16], sender_key: [u8;32], segment_bytes: u32, started: u64,
             sig: [u8;64] }
LogEntry = { seq: u64, segment_root: [u8;32], len: u32, prev_entry_hash: [u8;32] }
Frame    = { entry: LogEntry, sig: [u8;64] }
LogClose = { final_root: [u8;32], total_len: u64, closed: u64, last_entry_hash: [u8;32],
             sig: [u8;64] }
```

A Live ticket (section 4) carries `stream_id` and `sender_key`. The provider serves one `LogOpen`,
then one `Frame` per sealed segment, then one `LogClose` when the file is complete.

**Bytes.** The little-endian rule of section 2 applies, and there are no varints and no length
prefixes. Each record has a fixed length, and the fields come in the order of the table with no
padding, so a record has one spelling.

| Record | Fields and their bytes | Length |
|---|---|---|
| `LogOpen` | `stream_id` 16, `sender_key` 32, `segment_bytes` as a `u32` in 4, `started` as a `u64` in 8, `sig` 64 | 124 |
| `LogEntry` | `seq` as a `u64` in 8, `segment_root` 32, `len` as a `u32` in 4, `prev_entry_hash` 32 | 76 |
| `Frame` | the 76 bytes of its `LogEntry`, then `sig` 64 | 140 |
| `LogClose` | `final_root` 32, `total_len` as a `u64` in 8, `closed` as a `u64` in 8, `last_entry_hash` 32, `sig` 64 | 144 |

**Stream.** A record travels on the `sluice/1` connection after its length as a varint, and the
receiver reads one record at a time. The length MUST be exactly the one the table gives for that
record, so the three lengths 124, 140 and 144 are the only ones a receiver accepts, and the length
is how it knows which record it has. A sender MUST write every record on the same stream, in the
order **The chain** gives: the `LogOpen`, then the frames in `seq` order, then the `LogClose`.
Nothing else is written on that stream, and the close ends it.

`segment_bytes` is the size of a full segment. It is a multiple of 16,384 from 1,048,576 (1 MiB) to
67,108,864 (64 MiB), and a sender uses 8,388,608 (8 MiB) unless it is told another. 16,384 is the
block size of `iroh-blobs`, so a range inside a segment is a range of whole blocks. The lower bound
keeps the receiver's memory small: it keeps 32 bytes per entry, which is 32 MiB for a file of 1 TB
in segments of 1 MiB.

**Signatures.** Each `sig` is an Ed25519 signature under `sender_key` of this message:

1. the label of the record, in ASCII, from the table below
2. one byte `0x00`
3. the 16 bytes of `stream_id`
4. the bytes of the record before its `sig`

| Record | Label | Bytes signed after the `stream_id` |
|---|---|---|
| `LogOpen` | `sluice/live/open/1` | its first 60 bytes |
| `Frame` | `sluice/live/entry/1` | the 76 bytes of its `LogEntry` |
| `LogClose` | `sluice/live/close/1` | its first 80 bytes |

The label and `stream_id` make a signature good for one kind of record in one stream. A frame
signed for another stream, or the bytes of one kind of record signed as another, do not verify.
The same key signs receipts (section 10) under a label of their own, so no signature is good both
for a record here and for a receipt.

A receiver verifies strictly, as `verify_strict` of `ed25519-dalek` does: `s` MUST be below the
group order, neither `R` nor the key may be a point of small order, and the encoding of
`[s]B - [k]A` MUST equal the 32 bytes of `R`, with `k` as in RFC 8032 section 5.1.7. So a signature
has one spelling. A key of small order has signatures that hold for many messages, so under strict
verification it has none, and rule 3 refuses it before any signature is checked.

**The chain.** The hash of the `LogOpen` is the BLAKE3 hash of its first 60 bytes, without `sig`.
The hash of an entry is the BLAKE3 hash of its 76 bytes. The `seq` of the first entry is 0, and each
next one is one more. The `prev_entry_hash` of `seq` 0 is the hash of the `LogOpen`, and that of any
other entry is the hash of the entry before it. The `last_entry_hash` of the `LogClose` is the hash
of the last entry, or of the `LogOpen` when the stream has no entry. For a receiver, the head of the
chain is the hash of the last entry it accepted, or of the `LogOpen` before it accepts one.

`len` is the number of bytes of the segment, from 1 to `segment_bytes`. Every entry but the last has
`len` equal to `segment_bytes`. The last entry is only known as last when the `LogClose` arrives.

**Receiver.** A receiver keeps, for each accepted entry, the hash of its 76 bytes, and the sum of
their `len`. A record that a provider could have made, because it does not verify or comes too
early, is refused and changes nothing, so another provider can give the right one, and only a
record the sender signed can abort the stream.

The `LogOpen` comes first. A receiver MUST refuse it, and start no stream, when:

1. a record is not the length its table gives. This rule is for every record, and the receiver
   refuses it
2. the `LogOpen`'s `stream_id` or `sender_key` is not the ticket's
3. a signature does not verify, as **Signatures** says, under the ticket's `sender_key`. This rule
   is for every record, and the receiver refuses it. Under this rule the receiver also refuses the
   `LogOpen` when the ticket's `sender_key` is not in its one encoding or is of small order, as
   rule 8 of section 4 says, and checks it itself rather than trust that a decoder did. No
   signature verifies under such a key
4. `segment_bytes` is outside its bounds. It is checked after the signature

A frame is checked in this order:

5. a frame whose `seq` is past the next one is refused. Its signature is not checked. A provider
   that sends frames out of order loses nothing but the request
6. a frame whose `seq` is below the next one, and whose `LogEntry` hashes to the hash kept for that
   `seq`, is a repeat. The receiver drops it without checking its signature, and nothing changes
7. a frame whose `seq` is below the next one, with another `LogEntry`, is checked by rule 3. If it
   verifies, the sender signed two histories, through a leaked key or a bug, and the receiver
   aborts the stream at that `seq`. This is the fork rule

A frame at the next `seq` is checked by rule 3, and the receiver aborts the stream at that `seq`
when the frame verifies and:

8. its `prev_entry_hash` is not the head of the chain
9. its `len` is 0 or more than `segment_bytes`
10. the entry before it has a `len` below `segment_bytes`, so that entry was the last
11. the receiver has already accepted a `LogClose`

Any other frame at the next `seq` is accepted. The receiver keeps its hash, adds its `len` to the
sum, and hands the caller its `seq`, `segment_root` and `len`.

A `LogClose` is checked by rule 1 and rule 3, and then:

12. a `LogClose` whose `total_len` is more than the sum is refused when no entry is accepted yet or
    the last accepted one has a `len` of `segment_bytes`. The receiver is missing entries that the
    sender sealed, and it can offer the `LogClose` again once it has them
13. the receiver aborts the stream when `total_len` is not the sum in any other case: less than the
    sum, or more than it after an entry with a `len` below `segment_bytes`
14. the receiver aborts the stream when `last_entry_hash` is not the head of the chain
15. once a `LogClose` is accepted, the receiver aborts the stream for another validly signed one
    with other bytes. The sender signed two ends of one file

An abort for a `LogClose` is at the next `seq`. The sum is kept without wrapping.

A `LogClose` that breaks no rule closes the stream. The receiver hands the caller `final_root` and
`total_len`, and it cannot check `final_root` itself, because it has no bytes. The caller hashes
the file it assembled from the segments and MUST compare the result with `final_root` before it
treats the file as complete. From then on the content is ordinary static content, addressable and
dedupable as such.

**Abort.** An abort is a verification failure that names the `seq`. The receiver MUST keep only the
segments of entries before that `seq`, and MUST NOT accept an entry at or after it. After an abort
it answers every record with the same failure. After a close it accepts nothing. A frame that breaks
rule 7 or 11, or a `LogClose` that breaks rule 15, aborts the stream, and any other record is
answered with the close.

**Errors.** A receiver MUST NOT write a record it refused or aborted on, or any part of it, to a
log or an error message. The error names the rule, and the `seq` when there is one.

**Sender.** A sender MUST:

- write `segment_bytes` within its bounds, and sign each record as **Signatures** says, with the
  key the ticket names
- seal segments in order: sign the entry of `seq` n only after the entry of `seq` n - 1, set its
  `prev_entry_hash` by **The chain**, and never sign two entries for one `seq`
- give every entry a `len` from 1 to `segment_bytes`, and give a `len` below `segment_bytes` only
  to the last entry
- sign one `LogClose`, after the last entry and never before it, with `total_len` the sum of every
  `len`, `last_entry_hash` by **The chain**, and `final_root` the BLAKE3 hash of the whole file.
  It signs no entry after it

**Fetching.** Segment content is fetched by `segment_root` using the static protocol, so range
requests and resume work inside a segment. A receiver fetches the segment of an entry it has
accepted. A receiver that joins late MUST be able to request the chain from `seq` 0, and checks
every frame by the same rules.

**The tail.** A provider MAY serve the incomplete final segment at its current length, with its own
tree over that prefix. Its root changes as it grows and is therefore not part of the signed chain
until the segment is sealed. A receiver MUST treat tail bytes as provisional and MUST re-verify them
against the `segment_root` and `len` of the entry when it is accepted.

**Clock.** `started` and `closed` are signed, and no verification depends on them, because peer
clocks disagree.

---

## 7. Delta hints

Optional, capability `DELTA`. Purpose: resending a large file that changed slightly should not move
the whole file.

```
SigList = { algo: u8, avg_bytes: u32, entries: [SigEntry] }   // algo 1 = FastCDC v2020, 64-bit
SigEntry = { len: u32, strong: [u8; 16] }                     // BLAKE3-128 of the chunk
NeedRanges = { root: [u8;32], ranges: RangeSet }
```

Flow:

1. The **provider** sends `SigList` for the content the requester is already authorized to read.
   Signatures flow provider to requester, never the reverse, so the requester discloses nothing
   about its local files.
2. The requester chunks its own candidate files with the same parameters, matches strong hashes, and
   computes which byte ranges of the target it can reconstruct locally.
3. The requester sends `NeedRanges` for the remainder and fetches those with the static protocol.
4. The requester assembles and MUST verify the whole result against `root`. On failure it MUST
   discard the assembly and fall back to a plain fetch.

Delta is a hint layer. No verification outcome may ever depend on it.

---

## 8. Directory manifest

Capability `MANIFEST_V1`. A directory is a manifest blob plus the blobs it references. A manifest
comes from a stranger and tells the receiver where to write, so every rule below is checked before
anything is written.

```
Manifest = { version: u16, entries: [Entry], packs: [PackRef] }

Entry = {
  path: String,                // relative, NFC, forward slashes
  kind: u8,                    // 0 = File, 1 = Dir, 2 = Symlink
  size: u64,                   // bytes of content; 0 for Dir and Symlink
  mtime_ms: u64,               // UTC ms; 0 for a time before 1970
  mode: u32,                   // permission bits only, at most 0o7777; 0 when unknown
  loc: Loc,
  link_target: Option<String>, // present when kind is Symlink, absent otherwise
}

Loc = Blob { root: [u8;32] }                             // File: BLAKE3 of its content
    | Packed { pack_index: u32, offset: u64, len: u64 }  // File: a range of a pack
    | None                                               // Dir and Symlink
```

**Bytes.** The little-endian rule of section 2 does not apply inside a manifest: its bytes follow
the `postcard` 1.x encoding as stated here, so a third party needs no copy of postcard. The fields
come in this order, with no padding and no tag except where the table says so:

| Field | Bytes |
|---|---|
| `version` | a varint, `0x01` |
| `entries` | the count as a varint, then that many entries |
| one entry: `path` | the length in bytes as a varint, then that many bytes of UTF-8 |
| one entry: `kind` | 1 byte, `0x00` for File, `0x01` for Dir or `0x02` for Symlink |
| one entry: `size` | a varint |
| one entry: `mtime_ms` | a varint |
| one entry: `mode` | a varint |
| one entry: `loc` | the tag of the variant as a varint, `0x00` for Blob, `0x01` for Packed or `0x02` for None. For Blob, then `root` as 32 bytes with no length prefix. For Packed, then `pack_index`, `offset` and `len`, each as a varint. For None, nothing |
| one entry: `link_target` | `0x00` if absent. Otherwise `0x01`, the length in bytes as a varint, then that many bytes of UTF-8 |
| `packs` | the count as a varint, then for each pack `root` as 32 bytes and `size` as a varint |

A varint is LEB128: 7 bits per byte, the lowest group first, the high bit set on every byte but the
last. It is at most 3 bytes for a `u16`, 5 for a `u32` and 10 for a `u64`, and its value MUST fit
the integer. An encoder MUST write the shortest varint that holds the value.

The empty manifest is the three bytes `01 00 00`.

**Root.** The root of a manifest is the BLAKE3 hash of its bytes, which is the root `iroh-blobs`
gives the manifest as a blob. The `root` of a `Blob` is the BLAKE3 hash of the file's content, which
is the root `iroh-blobs` gives it as a raw blob.

**Hash sequence.** A Static ticket with `format` 1 names one more blob, the hash sequence. Its
content is 32-byte roots, in this order:

1. the manifest root
2. the root of every pack, in pack order
3. the root of every `Blob` `File` entry, in manifest order, each distinct root once

The ticket's `root` is the BLAKE3 hash of that sequence blob, and its `size` is the sum of the
`size` of every `File` entry. A receiver fetches the sequence, then the manifest, decodes it, and
computes the sequence the manifest yields. It MUST reject the transfer unless the two are equal:
no root may be missing, extra, repeated or in another place. It does this before the first content
request. An empty sequence, or one whose length is not a multiple of 32, is refused.

A sequence names the manifest before the content, so a receiver learns every path and every size
without fetching a file. A single file is sent as a manifest with one entry, which is why its name
travels with it. Format 0 is a raw blob for pipes and has no sequence.

**Decoding.** A receiver MUST reject a manifest when:

1. it is longer than the byte limit, 256 MiB unless the receiver sets another. This is checked
   before any byte is read. A receiver that is told the size of the manifest blob before it fetches
   it applies this rule to that size and fetches nothing
2. its entry count is over the entry limit, 1,000,000 unless the receiver sets another. This is
   checked before the first entry is read, and before any memory is set aside for entries
3. `version` is not 1. Nothing after it is read
4. a varint is longer than the shortest one for its value, or does not fit its integer
5. a `kind` is not 0, 1 or 2, a `loc` tag is not 0, 1 or 2, an `Option` tag is not 0 or 1, or a
   `path` or `link_target` is not UTF-8
6. the bytes end before the manifest does, or bytes remain after it. A count that the bytes left
   cannot hold is the first case
7. the paths are not in strictly ascending order, compared byte by byte. A repeated path is not in
   order, so the sort has no ties
8. an entry has a parent folder and no earlier entry has that path with `kind` Dir. The parent is
   the path up to its last `/`. A top-level entry, which has no `/`, has none
9. a `path` breaks a path rule below
10. `link_target` is absent on a Symlink or present on another kind
11. a `link_target` breaks a link target rule below
12. two paths collide, as the case collision rule below defines
13. `kind`, `loc` and `size` disagree. A File has a `Blob` or a `Packed` loc. A Dir or a Symlink has
    `None` and a `size` of 0
14. a `Packed` entry has a `pack_index` that is not below the number of packs, a `len` that is not
    its `size`, or an `offset` plus `len` that is past the `size` of its pack. The sum MUST NOT wrap
15. `mode` is above `0o7777`
16. the packs are not laid out end to end, so that every byte of a pack belongs to exactly one
    file and every pack has a byte. It is checked after rule 14, and it breaks one of these pack
    layout rules, taking the `Packed` entries in manifest order:
    1. the first has `pack_index` 0, and each later one has the `pack_index` of the one before it
       or that number plus one
    2. the first entry of a pack has `offset` 0, and each later one in the same pack starts where
       the one before it ends: its `offset` is the `offset` plus `len` of that one
    3. the last entry of each pack ends at that pack's `size`
    4. the number of packs is the last `pack_index` plus one, or 0 when no entry is `Packed`
    5. every pack has a `size` of at least 1. A pack of 0 bytes could hold only files of no bytes,
       which need no fetch

A decoder reads from front to back and MAY stop at the first rule it finds broken. A manifest that
breaks two rules is rejected either way, and the number a decoder reports for it is its own. The
vectors in `tests/vectors/manifests/` break one rule each.

Rule 16 does not ask that the sender packed as section 9 says. A small file that is a `Blob` of its
own breaks no rule, and nor does a `Packed` entry of no bytes at its place in a pack that has
bytes.

**Rejected manifests.** A decoder MUST NOT write a rejected manifest, or any path or link target in
it, to a log, an error message or a crash report. The error names the rule, and the path rule,
link target rule or pack layout rule inside it, and nothing of the text. A path comes from a
stranger, and file names stay out of logs.

**Path rules.** A receiver MUST reject a manifest path, on every platform, when:

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
9. a component starts with `.sluice-partial-` or `.sluice-tmp-`, the two prefixes a receive gives the
   store it writes beside the output and a folder it writes through. The prefixes themselves are
   reserved too. A sender MUST NOT put such a name in a manifest, and a receive writes its own stores
   under names no manifest can claim. The comparison first drops the code points a file system may
   ignore in a name (the Unicode default-ignorable ones, such as U+200B and U+FEFF) and reads
   U+0131 as `i`, because NTFS upcases the dotless `ı` to `I`; then it applies NFKC and full Unicode
   case folding, because Windows and macOS resolve a name regardless of case, macOS composes and
   decomposes it, and APFS folds some letters beyond ASCII. So `.SLUICE-PARTIAL-x`,
   `．sluice-partial-x`, `.ſluice-partial-x`, `.sluıce-partial-x` and the same name with a
   zero-width space after `slu` all name the same store there

With these rules no accepted path can leave the output root by its text alone, and a symlink already
on disk is the writer's concern, not these rules'.

**Link target rules.** A receiver MUST reject a `link_target` when:

1. it is empty, or longer than 4096 bytes in UTF-8
2. it begins with `/`, or ends with `/`, so an absolute target is never accepted
3. a component is empty or `.`. A `..` component is allowed, within rule 5
4. it breaks path rule 2, 5, 6, 7 or 8 above, applied the same way. Rule 6 rejects `\` and `:`, so
   `C:\x` and `\\server\share` are rejected too
5. resolved from the folder that holds the link, one component at a time, a `..` would step above
   the output root. So `a/link -> ../b` is accepted and `link -> ../x` is rejected

These rules look at the text only. A target that passes them can still reach another symlink on
disk, which is the writer's concern, as for paths.

**Case collision.** Two paths collide when they are equal after full case folding followed by NFC.
Full case folding maps every code point of the whole path by the entries of status `C` and `F` in
`CaseFolding.txt` and no other: `ß` becomes `ss` and `ﬃ` becomes `ffi`, and the Turkic entries of
status `T` and the simple entries of status `S` are not used. NFC comes last because a fold can
leave NFC: U+0390 folds to three code points, and U+03AA followed by U+0301 folds to two, and NFC
makes both the one code point U+0390. So `Readme.md` and `README.md` collide, and so do `straße` and
`STRASSE`.

Every pair of entries is compared, on every platform, whether or not the receiver's filesystem
ignores case. A folder `A` and a folder `a` collide, so `A/x` and `a/y` are refused through them.
The tables are those of Unicode 18.0.0, for the folding and for the NFC of path rule 2. A decoder
MUST use those and no other: a later version adds pairs, and a receiver on newer tables would refuse
a manifest that a sender on older ones built. A change of version is a change of this document, with
new vectors.

**Entries.**

- Entries MUST be sorted by `path`, byte by byte and strictly, so a manifest is deterministic and
  dedupable.
- Every folder under a top-level entry has a `Dir` entry, empty or not, so its `mode` and
  `mtime_ms` are kept. An entry comes after the `Dir` entry of its parent.
- `mtime_ms` is restored by default. Implementations MUST offer a way to skip it.
- `mode` holds permission bits only, at most `0o7777`: the nine bits of owner, group and others, and
  the setuid, setgid and sticky bits. It is applied on Unix. A receiver MUST NOT apply the setuid or
  setgid bit (`0o6000`) to anything it writes, whatever `mode` says, because a hostile sender could
  otherwise plant a setuid binary. A receiver MUST always keep owner read and write on a file it
  writes, so a `mode` without them gains them: `400` becomes `600` and `444` becomes `644`. Without
  them the receive could not read back a file it already wrote to compare it on a resume. A `mode` of
  `0` means the sender did not know one, and the file keeps the mode the receive makes it with, which
  has owner read and write too. On Windows only the read-only bit is honored, and a file is read-only
  when `0o200` is clear after that rule, so a received file is never read-only.
- **Symlinks are recorded and never followed during send.** A receiver MUST NOT create a symlink
  unless explicitly enabled.
- `link_target` is present exactly when `kind` is Symlink, which is rule 10.
- Hard links are not preserved. Implementations MUST document this.

**Top-level entries.** Every path given to `send` is one top-level entry: a file is a `File` entry
and a folder is a `Dir` entry with its contents beneath it, so `sluice send proj/` writes
`OUT/proj/...` and `sluice send a.mov photos/` writes `a.mov` and `photos/...`. A folder given on
its own therefore arrives inside a folder of its own name, as copying it in a file manager does.
Two arguments with the same final name are refused by the sender, which names both.

**Receiver writer rules.** A receiver MUST apply these to the manifest before it requests any
content blob:

1. A top-level entry whose name starts with `.` is refused unless the user explicitly allows hidden
   top-level names. Hidden names below the top are ordinary entries.
2. A top-level name that already exists in the output folder refuses the whole transfer unless the
   user asked to replace an existing path. The check uses the file system's own kind of each entry
   and never follows a link. An existing folder of that name counts as a clash. A resumed receive
   may write into a top-level name only when its partial store records that this receive made it;
   any other existing name is a clash like any other.
3. When replacing is allowed, a clashing file or link is replaced after its content verifies, and an
   existing folder of the same name is written into. A link at any target is replaced as an entry
   and never followed, so no byte is ever written through it. A folder is never replaced by a file
   or by a link, even when replacing is allowed: a rename cannot do it, and the transfer refuses
   that target before any content is fetched.
4. A file already at a target whose content matches the manifest exactly is skipped, which is how a
   resumed receive recognises what it wrote. Any other existing target without rule 2's permission
   is a clash, and the error names the first one.

A resume runs every check a first run runs, and the two rules above are the only differences. Its
partial store records the top-level names that run created, and no others: an entry the first run
found in the output belongs to the user and is a clash for a resume that is not allowed to replace
it. A store made by a run that was allowed to replace what was there records that too, and a resume
of it needs the same permission again. It never changes the mode or the modification time of a folder
that was already there when it wrote into it, and it never deletes a folder it did not make.

**The home's record of a partial store.** A receive keeps a record of each partial store it makes,
in the `partials` folder of the home, named by a random id that is also a line in the store's
marker. The record holds the store's absolute path and the ticket root. A receive treats a
`.sluice-partial-*` folder as its own only when a record names that folder and that root and the
marker carries the record's id; a folder with that name and no such record is refused with a
`Policy` error that names it, and is never read, changed or removed. The record is removed with the
store. A sender MUST NOT name a path that the path rules reserve for these folders.

**Sender.** A sender builds the entries from the folder it is given. It MUST:

- write each path as the NFC form of the name on disk, with `/` between components
- refuse the whole folder, and send nothing, when any name on disk is one a receiver would reject: a
  name that breaks a path rule, a case collision, a name that is not valid UTF-8, or two names that
  are the same path after NFC. It SHOULD list every such name with the rule it breaks, so that one
  run tells the user everything to fix, and it SHOULD do this before it hashes any file
- never follow a symlink, except the folder it was given when that is itself a link
- skip a symlink whose target breaks a link target rule, because the target means nothing on another
  machine, and a socket, a FIFO or a device. It SHOULD list what it skipped
- refuse the folder when a file cannot be read, and SHOULD list it as it lists a bad name
- refuse the folder when its manifest would break decoding rule 1 or 2 at the default limits,
  because a receiver on those limits would reject it. It SHOULD check the entry count before it
  hashes any file
- include hidden files, and have no ignore rules of its own
- treat hard links as separate files
- set the `loc` of every file and the list of packs by the packing algorithm of section 9
- write the BLAKE3 hash of the content as the `root` of a file that is a `Blob`, and hash a file
  that is packed into the root of its pack. For every file, packed or not, read the size and the
  modification time before it hashes it and again after, and fail when either changed, because
  the hash then names no version of the file
- write `0` as the `mode` of a symlink on every platform, and the permission bits of the file as
  they are on Unix. On Windows write `0o444` for a read-only file, `0o644` for another file and
  `0o755` for a folder
- write the modification time as UTC milliseconds, and `0` for a time before 1970

---

## 9. Small-file packs

Capability `PACKS`. Each blob costs a request and its own verification, and on a tree of many small
files that cost is most of the transfer. A sender joins small files end to end into pack blobs, so
a tree of 100,000 small files is a few blob transfers and not 100,000.

`PackRef = { root: [u8;32], size: u64 }`. The manifest lists the packs, and a `Packed` entry names
one by its position in that list.

**Constants.**

| Constant | Value |
|---|---|
| `PACK_THRESHOLD` | 1,048,576 bytes (1 MiB) |
| `PACK_TARGET` | 67,108,864 bytes (64 MiB) |

They are fixed and not settings. The same folder then gives the same packs and the same manifest
root on every machine, so a second send and a second sender of the same tree dedup.

**Algorithm.** A sender MUST set the `loc` of each File entry and the list of packs by these steps.
It walks the entries in manifest order, which is sorted by path, and keeps one open pack. The open
pack starts with no bytes and the index 0.

1. An entry that is not a File is skipped.
2. A file of 0 bytes is a `Blob`. Its `root` is the BLAKE3 hash of no bytes,
   `af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262`. It is never packed, so a
   folder of empty files makes no pack.
3. A file of `PACK_THRESHOLD` bytes or more is a `Blob`, and its `root` is the BLAKE3 hash of its
   content.
4. Any other file, of 1 to `PACK_THRESHOLD - 1` bytes, goes into the open pack. If the open pack
   has bytes, and its size plus the file's size is more than `PACK_TARGET`, the open pack is closed
   first, and a new open pack starts with no bytes and the next index.
5. The file is then `Packed { pack_index, offset, len }`, where `pack_index` is the index of the
   open pack, `offset` is the size of the open pack before the file and `len` is the file's size.
   The open pack grows by `len`.
6. After the last entry, the open pack is closed if it has bytes. The closed packs, in order of
   index, are the manifest's `packs`, each with its size in bytes.

A file is never split across packs, so a pack is at most `PACK_TARGET` bytes, and every pack has at
least one byte. A pack of exactly `PACK_TARGET` bytes is not closed early: step 4 closes a pack only
when the next file would take it past `PACK_TARGET`.

**Pack bytes and root.** The bytes of a pack are the contents of its files in manifest order, end
to end, with no header, padding or separator. Its `root` is the BLAKE3 hash of those bytes, which is
the root `iroh-blobs` gives the pack as a raw blob, so ranges, resume and verification work on a
pack as on any blob. A sender can hash a pack while it reads the files, without writing the pack to
disk or holding it in memory.

**Receiver.** Rule 16 of section 8 means every byte of a pack belongs to exactly one file, so a
receiver knows from the manifest which file each byte goes to.

- A receiver MAY fetch a whole pack into its per-transfer store and then copy each file's bytes
  out of it, or stream out of a pack while it fetches it. It MUST NOT stage a whole pack in memory,
  and MUST NOT write any file from a pack before that pack has verified.
- A pack that gives more bytes than its `size`, or ends before it, MUST fail.
- A file of 0 bytes needs no fetch, whether it is a `Blob` or a `Packed` entry.
- A receiver does not check that the sender followed the algorithm above, as section 8 says after
  rule 16.

---

## 10. Receipt

Capability `RECEIPTS`. A sender-signed statement that specific content was offered.

```
Receipt = { root: [u8;32], total_len: u64, sender_key: [u8;32], utc_ms: u64, sig: [u8;64] }
```

The fields come in this order with no padding, and the little-endian rule of section 2 applies, so
the four fields before `sig` are the first 80 bytes. `sig` is an Ed25519 signature under
`sender_key` of this message:

1. the label `sluice/receipt/1`, in ASCII
2. one byte `0x00`
3. the first 80 bytes of the receipt

This is the scheme of **Signatures** in section 6 without a `stream_id`, because a receipt names no
stream. A verifier verifies strictly, as section 6 says after **Signatures**. The label keeps a
receipt's signature from being good for a record of the live log, and a record's signature from
being good for a receipt.

A verifier refuses a receipt whose `sender_key` breaks rule 8 of section 4: it is not in its one
encoding, or it is a point of small order. Every key Sluice reads follows that one rule, so a key
that one part of an implementation refuses is never accepted by another.

A receipt proves the sender offered that content at that time. It does not prove delivery, and
implementations MUST NOT describe it as proof of delivery.

---

## 11. Control API

The local control interface is defined by `proto/sluice/v1`, governed by `buf`, and served over a
Unix socket or Windows named pipe. An implementation MAY also serve it over TCP, and then MUST
require TLS and token authentication.

It is a control plane. **No file bytes ever traverse this API.** A client asks the engine to move
bytes and observes progress; it never carries them.

The progress events an implementation reports are named here, in the order a transfer may report
them: `started`, `connected`, `path_changed`, `relayed`, `progress`, `retrying`, `warning`,
`verified`, `done`. `--json` writes one object per line with the name in `event`. `connected` names
the chosen path and, when it is relayed, the relay; `path_changed` reports every later move. Both are
taken back as soon as a direct path is in use. `relayed` is said at most once per connection, ten
seconds after it came up and only if the path is still relayed then, and names the relay and the
seconds waited; it is information and not a failure, and a path that goes direct after it is reported
by `path_changed` like any other move.

Store access is single-writer. A client MUST take an exclusive lock on the store directory, and if
the lock is held by a live process it MUST connect to that process instead of opening the store.

---

## 12. Test vectors

`tests/vectors/` contains, for each release:

| Vector | Contents |
|---|---|
| `tickets/` | Encoded tickets with expected decoded fields, including malformed ones that MUST be rejected |
| `manifests/` | Encoded manifests with expected fields and root, including ones that MUST be rejected |
| `packs/` | Files in manifest order with their sizes, and the `loc` of each and the size of each pack that section 9 gives. Some also give the contents, the pack roots and the manifest root |
| `sequences/` | A manifest and the hash sequence a ticket for it must name |
| `live/` | A ticket's `stream_id` and `sender_key`, the records of section 6 in the order a receiver gets them, the receiver's answer to each, and the state of the stream at the end, including streams that MUST be refused or aborted |
| `delta/` | File pairs with expected chunk boundaries and resulting need-ranges |
| `paths/` | Hostile paths that MUST be rejected by a receiver |
| `writer/` | Top-level names a receiver must accept, refuse as hidden, or refuse as a clash, with the flags and the output state that decide it |

`cargo xtask conformance` runs every vector against the local implementation. A third-party
implementation that passes these vectors is compatible with ours.

---

## 13. Versioning policy

- This document is versioned independently of the product.
- Wire-breaking changes bump the ALPN. Additive changes take a capability bit.
- CI runs interop of the current build against the previous release and the oldest supported
  release.
- The on-disk store layout is versioned separately in `$SLUICE_HOME/VERSION`, with forward-only
  migrations. An older binary MUST refuse a newer layout rather than write to it.

---

## 14. Open items before 0.2

1. Confirm the exact `iroh-blobs` protocol version to pin, and whether its pre-1.0 ALPN change has
   landed.
2. Measure the segment size curve. 8 MiB is a guess trading latency against signature overhead.
3. Measure the pack constants of section 9. They are fixed so that one folder gives one root, but
   the values are not measured. A change breaks no receiver, which does not check the algorithm, but
   a folder sent before and after it no longer dedups.
4. Decide whether `zstd` applies per chunk group or per pack. Per pack is simpler and probably
   enough.
5. Decide whether the tail-serving rule needs an explicit provisional-length field on the wire.
6. Settle section 5 rule 2. `iroh-blobs` fixes its block size at 16 KiB, so a 64 KiB chunk group
   cannot be negotiated while static content goes through it. Either the rule goes, or the block
   size becomes configurable upstream.
