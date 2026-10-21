# Test vectors

These files say what a Sluice implementation must accept and reject. `SPEC.md` section 12 lists the
kinds. An implementation that passes every vector here follows the same rules as ours at the points
the vectors cover. They do not cover the whole wire.

Run them against this repository with `cargo xtask conformance`. It exits non-zero if one case
disagrees, and `cargo nextest run` runs the same check.

## Layout

```
tests/vectors/
  paths/        manifest path rules, SPEC.md section 8
  tickets/      ticket text and bytes, SPEC.md section 4
  manifests/    directory manifests as bytes, SPEC.md section 8
  packs/        not written yet
  live/         not written yet
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

A ticket is a secret, and these are made-up ones: the keys are patterns such as `11` repeated, and the
addresses are in the documentation ranges (`192.0.2.0/24`, `198.51.100.0/24`, `2001:db8::/32`). A
case is a ticket text and a verdict. The text is a JSON string and not always canonical, so a
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
addresses, not as text. `relay_url` and `expiry_ms` are `null` when absent. `size` and `expiry_ms` are
decimal strings, because a JSON number does not hold a `u64` in every language.

To check an implementation, for every valid case decode `ticket`, compare the result with `fields`,
and encode the fields: the result must be `canonical`. For every rejected case, decoding `ticket` must
fail.

Where a rejected ticket can be a valid ticket with one fault, it is, so that fault is the reason it
fails. The files split them by where the fault is:

- `invalid-text.json`: the prefix, a character outside the alphabet (each of `i`, `l`, `o`, `u` in
  both cases, `-`, `=`, white space, a non-ASCII character), a body length that whole bytes cannot
  give, spare bits that are not zero, and text of 4097 bytes. The largest ticket that fits is 4095
  characters, because the base32 body can only end at certain lengths, and it is in `valid-static.json`.
- `invalid-bytes.json`: the bytes under well formed base32. An unknown version, kind or format, bytes
  after the body or missing from it, an empty `relay_url`, a tag that is neither value, text that is not
  UTF-8, and a varint with a zero byte too many.
- `invalid-relay.json`: a `relay_url` that breaks rule 7. A scheme that is not `https` or `http`, a
  relative URL, no scheme, a user name, a password, a query, a fragment, an upper-case scheme or
  host, no `/` as the path, a default port, a host that is not in punycode, an upper-case IPv6 host,
  a space, no host, and 2049 bytes. Rule 7 asks that the text equal its own serialization by the
  WHATWG URL standard, so a case can be refused by that comparison alone. `https:///x` is one: the
  standard skips the extra slash and reads `x` as the host.

`valid-relay.json` is the other side of rule 7: an `https` URL with a port, an `http` URL, an IPv6
and an IPv4 host, a path, a percent-encoded path and a URL of 2048 bytes. Its `canonical` is its
`ticket`, because a `relay_url` has one spelling. A `relay_url` that is present is always ASCII.

The bytes were written by a second encoder, built from the table in `SPEC.md` and not from the Rust
code, so the vectors check the spec and the implementation against each other.

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
| `sub_rule` | integer | Rejected cases of rule 9 and 11. The path rule or the link target rule inside it |
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
- Rule 12 folds the whole path. `A` and `a` are the same name, so `A/x` and `a/y` break it through
  their folders. `straße` and `STRASSE` break it through the full folding of `ß`, and the pair
  U+0390 and U+03AA U+0301 only through the NFC step that follows the fold.
- A path in a rejected case of rule 9 has no entry for its parent, so it breaks rule 8 as well. The
  case names rule 9, because a decoder reads the path before it looks for the parent. Whichever it
  names, it rejects the case, and the verdict is all a runner compares.

The files are `valid.json`, `limits.json`, `invalid-wire.json` (rules 3 to 6) and
`invalid-entries.json` (rules 7 to 15). The bytes and the roots were written by a second encoder in
Python, built from the table in `SPEC.md` section 8 with its own varints and its own BLAKE3, and not
from the Rust code, so the vectors check the spec and the implementation against each other.
