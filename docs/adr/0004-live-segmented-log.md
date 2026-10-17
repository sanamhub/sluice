# 0004. Live content is a signed segmented append-only log

**Status:** Accepted

## Context

An earlier draft specified live transfers as a "partial BLAKE3 root" published as the file grew. That is
not implementable: a bao tree's shape depends on total length, so a prefix of a growing file has no
stable root. The bug would have surfaced during implementation and forced a breaking protocol change
after users had tickets in circulation.

## Decision

Live content is a chain of fixed-size segments, default 8 MiB, each hashed as its own complete BLAKE3
tree. Entries carry `prev_entry_hash` and are signed by the sender key. The incomplete tail may be served
at its current length as provisional bytes, re-verified when the segment is sealed. At close the sender
publishes the whole-file root, after which the content is ordinary static content.

## Consequences

- Each segment is independently verifiable and readable the moment it lands.
- Range requests and resume work inside a segment, because a segment is a normal blob.
- Live content becomes dedupable and addressable after close, at no extra cost.
- Segment size is a real tuning knob: smaller means lower latency and more signatures. 8 MiB is a guess
  until measured.
