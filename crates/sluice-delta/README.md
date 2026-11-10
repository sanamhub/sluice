# sluice-delta

The delta hint layer of Sluice, as bytes in and values out. No file, no socket, no async: the engine
chunks the content it serves and the candidate a receiver already holds with the same parameters,
matches strong hashes, and fetches what is left (ADR 0005, ADR 0008).

## What is here

- `Params`: the chunking parameters of `SPEC.md` section 7. An average that is a power of two from
  256 bytes to 4 MiB, a minimum of a quarter of it and a maximum of eight times it.
- `Params::chunks`: FastCDC v2020 with a 64-bit gear hash. The gear table is the first eight bytes,
  little-endian, of `BLAKE3("sluice/delta/gear/1" || [i])` for entry `i`, so every implementation
  computes the same table from the same sentence.
- `SigList`, `Entry`: the chunk lengths and the first 16 bytes of each chunk's BLAKE3 hash, as the
  provider sends them.
- `NeedRanges`, `Range`: the target ranges a requester still needs, ascending and apart.
- `Candidate`: the chunks of the file the requester already holds, cut with the same parameters.
- `plan`: the matcher. It takes the provider's `SigList` and the requester's `Candidate` and returns
  a `Plan`, which is the runs of the target that come from the candidate and the runs that have to
  be fetched, in order and with each byte covered once. Adjacent runs are merged, so a plan asks for
  one range where it would have asked for a thousand.
- `Plan::need_ranges` turns the fetched runs into the message that asks for them, and
  `Plan::rebuild` assembles the target from the candidate and the fetched bytes. That is the
  reference the engine's own assembly is checked against.

## What it does not do

It does not decide that a result is correct. Sixteen bytes of hash can be made to collide on purpose
by a peer, so a matched chunk is a guess that saves bytes and the whole assembled file is verified
against its BLAKE3 root afterwards. A wrong or hostile hint costs time and nothing else.

It also does no IO and holds no state between calls: the caller reads the files and writes the
bytes. That is what lets the same code run in the browser build and be tested as a function call.

## Tests

Unit tests cover the parameters, both codecs and every refusal a hostile message must meet. The
property tests cover the promise the whole feature rests on: every byte lands in exactly one chunk,
the same bytes cut at the same places, every chunk stays within its bounds, and one changed byte
moves at most two boundaries.
