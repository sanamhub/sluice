# 0004. Live content is a signed log of fixed-size segments

**Status:** Proposed
**Date:** 2026-10-19
**Approved by:** pending

## Context

Sending a file while it is still being written (a recording, a database dump, a long-running export)
needs verification before the file is finished. A bao tree's shape depends on the total length, so a
growing file has no stable BLAKE3 root to verify against. Publishing a "partial root" as the file grows
does not work for the same reason.

Hypercore solves a similar problem with a signed append-only Merkle log, and has done so in production
for years. It defines its own block format, though, while we want live content to become an ordinary
BLAKE3 blob once it is complete.

## Decision

Live content is a chain of fixed-size segments, 8 MiB by default:

- Each segment is hashed as its own complete BLAKE3 tree.
- Each log entry records its sequence number, segment root, length and the hash of the previous entry,
  and is signed with the sender's key.
- The unsealed tail may be served at its current length. Receivers treat it as provisional and verify
  it again when its entry arrives.
- On close, the sender publishes the whole-file root, and the content becomes ordinary static content.

The exact rules are in `SPEC.md` section 6.

## Consequences

- A segment is readable as soon as it arrives, and verifies on its own.
- Resume and range requests work inside a segment, because a segment is a normal blob.
- After close, the file deduplicates and addresses like any other.
- Segment size trades latency against signature overhead. 8 MiB is unmeasured.
- The spec does not yet say what happens when a sender signs two different entries for one `seq`.
  Hypercore's fork handling is the reference to study before that rule is written.
