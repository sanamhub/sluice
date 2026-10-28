# Test vectors

These files say what a Sluice implementation must accept and reject. `SPEC.md` section 12 lists the
kinds. An implementation that passes every vector here follows the same rules as ours at the points
the vectors cover. They do not cover the whole wire.

The vectors were written from `SPEC.md`, not from the Rust code, so they check the spec and the
implementation against each other.

Run them against this repository with `cargo xtask conformance`. It exits non-zero if one case
disagrees, and `cargo nextest run` runs the same check.

## Layout

```
tests/vectors/
  paths/        manifest path rules, SPEC.md section 8
  tickets/      ticket text and bytes, SPEC.md section 4
  manifests/    directory manifests as bytes, SPEC.md section 8
  packs/        small-file packing, SPEC.md section 9
  sequences/    the hash sequence of a folder ticket, SPEC.md section 8
  writer/       the top-level rules a receiver applies before it writes, SPEC.md section 8
  live/         the signed segment log, SPEC.md section 6
  delta/        not written yet
```

One directory per row of the table in section 12. A directory holds one or more `.json` files. The
runner reads every `.json` file in a directory it knows, in name order, and reports a directory it
does not know as skipped, never as passed. A directory named here but missing from the tree has no
vectors yet.

## File format

Every file is one JSON object, UTF-8, with two fields:

| Field | Type | Meaning |
|---|---|---|
| `description` | string | What the cases in the file have in common. A runner prints it next to a failing case |
| `cases` | array | The cases. Never empty. Their order carries no meaning |

The shape of a case depends on the kind. A reader must ignore fields it does not know, so a later
release can add a field without breaking an older runner.

## `paths/`

A case is one manifest path and the verdict a receiver must reach:

```json
{ "path": "a/../b", "valid": false, "rule": 4 }
{ "path": "photos/IMG_0001.jpg", "valid": true }
```

| Field | Type | Meaning |
|---|---|---|
| `path` | string | The path exactly as a manifest carries it |
| `valid` | boolean | `true` if a receiver MUST accept it, `false` if it MUST reject it |
| `rule` | integer | Present when `valid` is `false`. The number of the rule in `SPEC.md` section 8 that the path breaks |

To check an implementation, give every `path` to its receiver check and compare the verdict with
`valid`. That is all the runner in this repository compares.

If a path breaks more than one rule, `rule` is the lowest number. The file `lowest-rule.json` holds
such paths. Compare `rule` only if your implementation reports the rule it stopped at, and then
expect the lowest, not the first one met when reading left to right.

Some points a reader cannot see from the rules alone:

- The vectors test the text of a path and nothing on disk. A symlink that already exists in the
  output folder is the writer's concern.
- Rule 1 and rule 5 count bytes of UTF-8, not characters. A component of 128 `é` is 256 bytes and
  breaks rule 5.
- Rule 2 is a check. An implementation must not normalize a path and then accept it.
- Rule 8 takes the name from the text before the first `.`, ignores spaces before that dot, and
  compares U+0131 as `I`. So `con.tar.gz` and `CON .txt` break rule 8, and `a.con` does not.
- Rule 6 rejects the C1 and bidirectional controls and accepts the zero-width characters U+200B,
  U+200C, U+200D and U+FEFF. A UI that shows a name has to escape them.
- A path with control characters, decomposed accents, ligatures or look-alike characters is written
  with `\u` escapes, so the file shows what it holds. Read the JSON with a parser, not by eye.
- A path is a JSON string, so it is valid Unicode. A manifest decoder rejects bytes that are not
  UTF-8 before it asks the path rules anything, and no vector covers that.
- Each file is named for the rule it tests. It holds the paths that break that rule, and next to
  them the closest paths that must pass, such as `..a` for rule 4 and `CONSOLE` for rule 8.
  `accepted.json` holds ordinary paths.

## `tickets/`

A ticket is a secret, and these are made-up ones: the keys are patterns such as `11` repeated, and
the addresses are in the documentation ranges (`192.0.2.0/24`, `198.51.100.0/24`, `2001:db8::/32`).
A case is a ticket text and a verdict. The text is a JSON string and not always canonical, so a
decoder must be given it exactly.

A valid case says what the ticket decodes to and what an encoder writes for it:

```json
{
  "ticket": "SLUICE1...",
  "valid": true,
  "canonical": "sluice1...",
  "fields": {
    "version": 1,
    "kind": "static",
    "endpoint_id": "1111...",
    "addrs": ["192.0.2.1:50000", "[2001:db8::1]:443"],
    "relay_url": null,
    "expiry_ms": null,
    "root": "2222...",
    "size": "104857600",
    "format": "raw"
  }
}
```

A rejected case has a `reason` for people, which a runner does not compare:

```json
{ "ticket": "sluice2...", "valid": false, "reason": "the wrong prefix" }
```

| Field | Type | Meaning |
|---|---|---|
| `ticket` | string | The text given to the decoder |
| `valid` | boolean | `true` if a decoder MUST accept it, `false` if it MUST reject it |
| `canonical` | string | Valid cases. The text an encoder writes for `fields`, in lower case. It can differ from `ticket` only in case |
| `fields` | object | Valid cases. What the ticket decodes to |
| `reason` | string | Rejected cases. What is wrong with it |

In `fields`, `kind` is `static` or `live`. A static ticket has `root`, `size` and `format` (`raw` or
`hash_sequence`). A live ticket has `stream_id` and `sender_key`. Keys and ids are lower-case hex.
`addrs` are written as `ip:port`, with the IPv6 address in square brackets, and are compared as
addresses, not as text. `relay_url` and `expiry_ms` are `null` when absent. `size` and `expiry_ms`
are decimal strings, because a JSON number does not hold a `u64` in every language.

To check an implementation, for every valid case decode `ticket`, compare the result with `fields`,
and encode the fields: the result must be `canonical`. For every rejected case, decoding `ticket`
must fail.

Where a rejected ticket can be a valid ticket with one fault, it is, so that fault is the reason it
fails. The files split them by where the fault is:

- `invalid-text.json`: the prefix, a character outside the alphabet (each of `i`, `l`, `o`, `u` in
  both cases, `-`, `=`, white space, a non-ASCII character), a body length that whole bytes cannot
  give, spare bits that are not zero, and text of 4097 bytes. The largest ticket that fits is 4095
  characters, because the base32 body can only end at certain lengths, and it is in
  `valid-static.json`.
- `invalid-bytes.json`: the bytes under well formed base32. An unknown version, kind or format,
  bytes after the body or missing from it, an empty `relay_url`, a tag that is neither value, text
  that is not UTF-8, and a varint with a zero byte too many.
- `invalid-keys.json`: an `endpoint_id` or a `sender_key` that breaks rule 8, each in a Live ticket.
  A second spelling of a point (`y` of `p + 3`, `2^255 - 1` and `p + 1`, and the sign bit set on the
  neutral point, whose `x` is 0), a point of order 1, 4 and 8, and 32 bytes that are no point. A
  ZIP-215 decoder, such as `ed25519-dalek`'s, reads the second spellings as points, and the first
  two are of large order, so rule 8 is the only reason they fail.
- `invalid-relay.json`: a `relay_url` that breaks rule 7. A scheme that is not `https` or `http`, a
  relative URL, no scheme, a user name, a password, a query, a fragment, an upper-case scheme or
  host, no `/` as the path, a default port, a host that is not in punycode, an upper-case IPv6 host,
  a space, no host, and 2049 bytes. Rule 7 asks that the text equal its own serialization by the
  WHATWG URL standard, so a case can be refused by that comparison alone. `https:///x` is one: the
  standard skips the extra slash and reads `x` as the host.

Rule 8 is why the keys are only some patterns: `44` repeated is no point, so the sender key of the
Live cases is `55` repeated. A `root` is no key, and one case in `valid-static.json` has `44`
repeated as its `root`.

`valid-relay.json` is the other side of rule 7: an `https` URL with a port, an `http` URL, an IPv6
and an IPv4 host, a path, a percent-encoded path and a URL of 2048 bytes. Its `canonical` is its
`ticket`, because a `relay_url` has one spelling. A `relay_url` that is present is always ASCII.

## `manifests/`

A receiver reads a manifest before it writes anything, so these vectors are manifest bytes and not
directory trees. A case is the bytes as hex and a verdict:

```json
{ "name": "one file", "bytes": "0101...", "valid": true, "root": "2f26...", "fields": { ... } }
{ "name": "version 2", "bytes": "020000", "valid": false, "rule": 3, "reason": "version is not 1" }
```

| Field | Type | Meaning |
|---|---|---|
| `name` | string | What the case is. A runner prints it next to a failure and does not compare it |
| `bytes` | string | The manifest as lower-case hex. It can be empty: a manifest cut after 0 bytes |
| `valid` | boolean | `true` if a decoder MUST accept it, `false` if it MUST reject it |
| `root` | string | Valid cases. The BLAKE3 hash of `bytes`, as hex |
| `fields` | object | Valid cases. What `bytes` decode to |
| `rule` | integer | Rejected cases. The decoding rule of `SPEC.md` section 8 that the bytes break |
| `sub_rule` | integer | Rejected cases of rule 9, 11 and 16. The path rule, the link target rule or the pack layout rule inside it |
| `reason` | string | Rejected cases. What is wrong with it, for people |
| `limits` | object | Optional. `max_bytes` and `max_entries`, decimal strings, in place of the defaults |

In `fields`, `entries` and `packs` are lists in manifest order. An entry has `path`, `kind` (`file`,
`dir` or `symlink`), `size`, `mtime_ms`, `mode`, `loc` and `link_target` (`null` when absent). A
`loc` is `{"type": "blob", "root": hex}`, `{"type": "packed", "pack_index": n, "offset": s, "len":
s}` or `{"type": "none"}`. A pack has `root` and `size`. Sizes, times, offsets and lengths are
decimal strings, because a JSON number does not hold a `u64` in every language. `mode` and
`pack_index` are numbers. Roots are lower-case hex.

To check an implementation, decode `bytes` with the limits of the case, or the defaults if there are
none. For every valid case, compare the result with `fields`, encode it, and compare the bytes with
`bytes` and the root with `root`. For every rejected case, decoding must fail. Compare `rule` only
if your implementation reports the rule it stopped at: a case has one fault, so there is one to
report.

Some points a reader cannot see from the rules alone:

- The limits of rule 1 and 2 are 256 MiB and 1,000,000 entries, and no vector is that big. A case
  with a `limits` object tests the same check with small numbers. Two cases with no `limits` object
  use the default entry limit: a count of 1,000,001 is rejected by rule 2, and a count of 1,000,000
  with a few bytes behind it by rule 6, because the bytes cannot hold it.
- The byte limit is checked first. A case of garbage bytes over `max_bytes` breaks rule 1, whatever
  the garbage is.
- The folder that was sent is not an entry, and every folder inside it is. `a/b` needs an entry `a`
  of kind `dir` before it.
- Entries are in byte order, so `a`, `a-b`, `a/b` is sorted (`-` is 0x2D and `/` is 0x2F) and `a`,
  `B` is not.
- Rule 12 compares whole paths by the comparison key of path rule 9. `A` and `a` are the same name,
  so `A/x` and `a/y` break it through their folders. `straße` and `STRASSE` break it through the
  full folding of `ß`, and `i` and `ı` break it because the key reads the dotless i as `i`.
- A path in a rejected case of rule 9 has no entry for its parent, so it breaks rule 8 as well. The
  case names rule 9, because a decoder reads the path before it looks for the parent. Whichever it
  names, it rejects the case, and the verdict is all a runner compares.

- Rule 16 is about the packs as a whole, so a manifest breaks it only through its pack list and the
  `Packed` entries together. A case that breaks rule 14 also breaks rule 16 when the range runs past
  its pack, and names rule 14, which is checked first. A small file that is a blob of its own, and a
  `Packed` entry of no bytes at its place in a pack that has bytes, are valid. A pack of 0 bytes
  breaks pack layout rule 5, even when the files in it are all of no bytes.

The files are `valid.json`, `limits.json`, `invalid-wire.json` (rules 3 to 6) and
`invalid-entries.json` (rules 7 to 16). The bytes and roots come from a second encoder in Python,
built from the table in `SPEC.md` section 8 with its own varints and its own BLAKE3.

## `packs/`

These vectors test the packing algorithm of `SPEC.md` section 9: where a sender puts each file. A
case is the entries of a manifest in manifest order, and the layout the algorithm gives them:

```json
{
  "name": "one small file",
  "entries": [
    { "path": "hello.txt", "kind": "file", "size": "5",
      "loc": { "type": "packed", "pack_index": 0, "offset": "0", "len": "5" } }
  ],
  "packs": [ { "size": "5" } ]
}
```

| Field | Type | Meaning |
|---|---|---|
| `name` | string | What the case is. A runner prints it next to a failure and does not compare it |
| `entries` | array | The entries in manifest order. Each has `path`, `kind` (`file` or `dir`) and `loc`, and a file has `size` |
| `packs` | array | The packs in order of index. Each has `size`, and `root` in a case with content |
| `manifest_root` | string | Cases with content. The BLAKE3 hash of the manifest's bytes, as hex |

A `loc` is `{"type": "packed", "pack_index": n, "offset": s, "len": s}`, `{"type": "blob"}` or
`{"type": "none"}` for a folder. Sizes, offsets and lengths are decimal strings.

Most cases have sizes and no content, so a boundary at 64 MiB needs no 64 MiB of bytes. A case with
content also gives, for every entry, `mtime_ms` (a decimal string) and `mode` (a number), and for a
file its `content` as lower-case hex. Its blob locs carry `root`, the BLAKE3 hash of the content.
The bytes of a pack are the contents of its files end to end, and its `root` is their BLAKE3 hash.
`manifest_root` is the root of the manifest of those entries and packs, encoded as section 8 says.

To check an implementation, give the sizes in order to its packing step, with no size for a folder,
and compare every `loc` and every pack `size`. For a case with content, compare the roots too. The
runner in this repository also splits each pack back into files and compares each one with its
content.

The files are `boundaries.json` (the threshold, the target, empty files and no files), `mixed.json`
(small, large and empty files together, nested folders, and layouts of two and three packs) and
`content.json`. They were written by a second planner in Python, built from the steps in section 9
with the manifest encoder and the BLAKE3 of the `manifests/` vectors, and not from the Rust code.

## `live/`

These vectors test a receiver of the live log in `SPEC.md` section 6: what it answers to each record
a provider gives it. A case is the `stream_id` and `sender_key` of a Live ticket, the records in the
order they arrive, and where the stream stands at the end:

```json
{
  "name": "a frame ahead of the next one, then the frames in order",
  "stream_id": "5a5a...",
  "sender_key": "d75a...",
  "steps": [
    { "record": "open", "bytes": "5a5a...", "outcome": "started" },
    { "record": "frame", "bytes": "0100...", "outcome": "refused", "rule": 5 },
    { "record": "frame", "bytes": "0000...", "outcome": "accepted", "seq": "0",
      "segment_root": "...", "len": "1048576" }
  ],
  "final": { "state": "open", "next_seq": "2" }
}
```

| Field | Type | Meaning |
|---|---|---|
| `name` | string | What the case is. A runner prints it next to a failure and does not compare it |
| `stream_id`, `sender_key` | string | The ticket's, as lower-case hex |
| `steps` | array | The records in order. Each has `record` (`open`, `frame` or `close`), `bytes` as hex, and `outcome` |
| `final` | object | Where the stream stands after the last step. Its `state` is `not started`, `open`, `closed` or `aborted` |

An `outcome` is one of these, with the fields that go with it:

| `outcome` | Fields | Meaning |
|---|---|---|
| `started` | | The `LogOpen` passed rules 1 to 4 |
| `accepted` | `seq`, `segment_root`, `len` | The frame is the next entry of the chain |
| `repeat` | `seq` | The frame repeats an accepted entry and was dropped |
| `refused` | `rule` | The record was refused and changed nothing |
| `closed` | `final_root`, `total_len` | The stream is closed, or was already |
| `aborted` | `seq`, `rule` | The stream is aborted at `seq` |

In `final`, `open` has `next_seq`, `closed` has `final_root` and `total_len`, and `aborted` has
`seq` and `rule`. `rule` is the number of the receiver rule in section 6. Sequence numbers and
lengths are decimal strings, because a JSON number does not hold a `u64` in every language.

To check an implementation, start a receiver from the ticket's fields and the first `open` step that
it accepts, give it every later step in order, and compare each answer and the state at the end. A
step `open` comes only before the stream has started: an open that is refused starts nothing, and a
later `open` step can give another one.

The records are signed with the secret key of RFC 8032 section 7.1 TEST 1,
`9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60`, which is published. Its public
key `d75a...511a` is the `sender_key` of most cases. No other secret key is used. The cases with a
key of small order use the neutral point `0100...00` as `sender_key`, and the cases with a sender
that is not the ticket's use the public key of TEST 2.

Some points a reader cannot see from the rules alone:

- Each case breaks one rule, and the steps after the fault show what it changed: a refused record is
  followed by the right one, which is accepted, and an aborted stream answers the next records with
  the same abort.
- A frame ahead of the next `seq` is refused before its signature is checked, so a frame with no
  valid signature far ahead is refused by rule 5 and not rule 3.
- A repeat is found by the hash of the entry, before the signature, so an accepted frame with a
  damaged signature is a repeat.
- Two cases have a signature that the group equation `[s]B = R + [k]A` accepts and strict
  verification refuses: one with `s + L` in place of `s`, and one under the neutral point with `R`
  the neutral point and `s` 0, which holds for every message.
- A close before the entries it counts is refused by rule 12, and accepted once they have arrived.
- One case has a `sender_key` out of its one encoding, `y = 2^255 - 1`, which a ZIP-215 decoder
  reads as the point whose `y` is 18. The TEST 1 key has no second spelling: its `y` is far above
  18, so `y + p` does not fit in 255 bits, and its `x` is not 0. Nobody knows the secret of this
  point, so the open's signature cannot verify either, and the case cannot show which check refused
  it. The unit tests of both crates and `tickets/invalid-keys.json` do.
- The fork cases are in `aborted.json`, at `seq` 0 and at a later `seq`.

The files are `clean.json` (whole chains), `open.json` (the opening record and every bound of
`segment_bytes`), `refused.json`, `aborted.json`, `repeats.json` and `lengths.json` (every record
one byte short and one byte long). A second implementation in Python wrote them, from section 6 with
its own Ed25519 from RFC 8032, checked against the test vectors of its section 7.1, and the BLAKE3
of the `manifests/` vectors.

## `sequences/`

These vectors test the hash sequence a Static ticket with `format` 1 names. A case gives a
manifest and the bytes of its sequence blob:

```json
{ "name": "one file, one blob", "manifest": "0101...", "sequence": "2f26...abab...", "valid": true }
```

| Field | Type | Meaning |
|---|---|---|
| `name` | string | What the case is. A runner prints it next to a failure and does not compare it |
| `manifest` | string | The manifest as lower-case hex |
| `sequence` | string | The sequence blob as lower-case hex |
| `valid` | boolean | `true` if the sequence is the one the manifest yields, `false` if a receiver MUST refuse it |
| `reason` | string | Rejected cases. What is wrong with it, for people |

To check an implementation, decode `manifest`, compute the sequence its rules give, and compare the
bytes with `sequence`. A receiver must refuse the whole transfer when they differ. The rules are the
manifest root first, the pack roots in pack order, then every distinct `Blob` root of a `File` entry
in manifest order. The sequence is the ticket's `root`; this directory checks the bytes, not the
ticket that carries their hash.

## `writer/`

The `writer/` cases cover the receiver's rules for a top-level name, which `SPEC.md` section 8
states after the entries: a hidden name needs `--allow-hidden`, and a name already in the output
folder is a clash unless the caller asked to replace it or the transfer is resuming. A case is:

| Field | Type | Meaning |
|---|---|---|
| `name` | string | A path from the manifest |
| `exists` | bool | Something already stands at that top-level name in the output |
| `allow_hidden` | bool | `--allow-hidden` was given |
| `replace` | bool | `--force` was given |
| `resume` | bool | An interrupted receive of the same ticket is continuing |
| `expect` | string | `accept`, `hidden` or `clash` |

Only the top-level name is a rule of this kind. A nested path is accepted here; its ancestors and
its own target are checked against the output folder, which needs a file system and is covered by
the engine tests.
