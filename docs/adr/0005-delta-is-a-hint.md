# 0005. Delta is a hint layer that cannot affect correctness

**Status:** Accepted

## Context

Verified streaming needs deterministic chunk boundaries, so the BLAKE3 tree uses fixed 16 KiB groups.
Delta transfer needs boundaries that shift with insertions, which is what content-defined chunking
provides. One chunker cannot satisfy both requirements.

An earlier draft also had the receiver sending signatures of its own local files to the sender, which
leaks what the receiver already has on disk.

## Decision

Keep the BLAKE3 tree as the only integrity and addressing structure. Add a FastCDC index purely as a
hint. The provider publishes signatures for content the requester is already authorized to read; the
requester matches locally and asks only for what it cannot reconstruct. Every assembled result is
verified against the real root, and a failed verification falls back to a plain fetch.

## Consequences

- Integrity never depends on the CDC layer, which is what makes having it safe.
- Signature flow is provider to requester only, so the requester discloses nothing.
- The worst case for a bad hint is wasted work, never a corrupt file.
